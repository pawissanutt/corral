import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { CredentialStore } from "@earendil-works/pi-ai";

import { HOST_PROVIDER_IDS, hostRules } from "../src/auth/providers.ts";
import { EventLog } from "../src/bridge/events.ts";
import type { HostCapabilities } from "../src/host/config.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const capabilities: HostCapabilities = {
  catalog: 4, digest: "a".repeat(64), preset: "standard", enabled: ["llama_cpp.enabled"],
  llama_cpp: {
    endpoint: "http://127.0.0.1:8080", discover: true, context_window: 32768, max_tokens: 1024, timeout_ms: 15000,
  },
};

const routes = [
  { provider: "openai-codex", model: "gpt-6.1-sol" },
  { provider: "anthropic", model: "claude-opus-5-5" },
  { provider: "amazon-bedrock", model: "us.anthropic.claude-sonnet-4-5-20250929-v1:0" },
] as const;

for (const role of ["worker", "reviewer", "pm"] as const) {
  for (const { provider, model } of routes) {
    test(`${role}/${provider} creates its cloud session when the role also permits local inference`, async () => {
      // Given a real host runtime, cloud credentials and a local-capable role snapshot.
      const work = mkdtempSync(join(tmpdir(), "corral-pi-mixed-host-"));
      const assets = buildFixtureInstall();
      const originalFetch = globalThis.fetch;
      let host: HostSession | undefined;
      try {
        restrictEnvironment(process.env, provider);
        const sdk = await import("@earendil-works/pi-coding-agent");
        const modules = await loadModules(assets.dir, role);
        const transport = fakeCodexTransport();
        const network = guardedFetch(transport.send, undefined, provider, hostRules(provider, {
          region: "us-west-2", model, assumesRole: false, ssoRegions: [],
        }));
        globalThis.fetch = network.fetch;
        const cloudCredentials: CredentialStore = provider === "openai-codex"
          ? modules.auth.credentials(authContext(provider)) : {
            read: async (id) => id === "anthropic" ? {
              type: "oauth", access: "sk-ant-oat-fixture", refresh: "fixture", expires: 4_000_000_000_000,
            } : undefined,
            list: async () => provider === "anthropic" ? [{ providerId: provider, type: "oauth" }] : [],
            modify: async () => { throw new Error("fixture credentials are read-only"); },
            delete: async () => { throw new Error("fixture credentials are read-only"); },
          };
        const credentialContexts: string[] = [];
        const identity = { store: "st-demo", project: "demo", role, generation: 1, task: "T-578", run: "R1" };
        const events = new EventLog(1);
        const corral = writeFakeCorral(work);
        const client = new CorralClient({
          corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
        });
        const keys = { accessKeyId: "AKIAFIXTURE", secretAccessKey: "fixture" };
        // When session creation and reload select the explicitly bound cloud provider.
        host = await HostSession.create({
          sdk, identity, cwd: work, agentDir: join(work, "agent"), provider, model, capabilities, client, events, network,
          modules: { ...modules, auth: {
            ...modules.auth, providers: HOST_PROVIDER_IDS,
            credentials(context) { credentialContexts.push(context.provider); return cloudCredentials; },
          } },
          ...(provider === "amazon-bedrock" ? { aws: {
            binding: { profile: "fixture", region: "us-west-2" },
            resolver: {
              get: async () => keys,
              profile: async () => ({
                root: { kind: "static", profile: "fixture", credentials: keys }, assumesRole: false, ssoRegions: [],
              }),
            },
          } } : {}),
          sessionManager: sdk.SessionManager.create(work, join(work, "sessions")),
        });
        await host.session.bindExtensions({});
        await host.session.reload();
        // Then cloud model/auth/transport selection survives without local catalog traffic.
        expect(host.state().model).toBe(`${provider}/${model}`);
        expect(host.state().capabilities).toEqual(capabilities);
        expect(credentialContexts).toEqual([provider]);
        expect(transport.requests).toBe(0);
        await expect(network.fetch("http://127.0.0.1:8080/v1/models")).rejects.toThrow(/network_refused/);
        if (provider === "openai-codex") {
          await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
          const ended = nextEvent(events, (event) => event.kind === "turn_ended");
          expect((await host.submit({
            text: "Reply only CLOUD.", busyPolicy: "prompt", idempotencyKey: role, origin: "test",
          }, role)).disposition).toBe("started");
          await ended;
          await host.drained();
          const answer = host.session.messages.find((message) => message.role === "assistant");
          expect(answer?.role === "assistant" && answer.provider).toBe("openai-codex");
          expect(answer?.role === "assistant" && answer.stopReason).toBe("stop");
          expect(transport.requests).toBe(1);
        }
      } finally {
        host?.session.dispose();
        globalThis.fetch = originalFetch;
        assets.cleanup();
        rmSync(work, { recursive: true, force: true });
      }
    }, 60_000);
  }
}

test("session creation still refuses a local provider without explicit permission and settings", async () => {
  // Given a local provider selected directly, bypassing the launch parser.
  const work = mkdtempSync(join(tmpdir(), "corral-pi-unconfigured-local-"));
  const assets = buildFixtureInstall();
  try {
    const sdk = await import("@earendil-works/pi-coding-agent");
    const modules = await loadModules(assets.dir, "worker");
    const identity = { store: "st-demo", project: "demo", role: "worker" as const, generation: 1 };
    const { llama_cpp: _settings, ...withoutSettings } = capabilities;
    const network = guardedFetch(async () => { throw new Error("unconfigured local inference must not send a request"); });
    for (const local of [undefined, withoutSettings, { ...capabilities, enabled: [] }]) {
      // When the host is constructed, then it refuses before credentials or catalog access.
      await expect(HostSession.create({
        sdk, identity, cwd: work, agentDir: join(work, "agent"), provider: "llama.cpp", model: "local-model",
        ...(local ? { capabilities: local } : {}),
        modules: { ...modules, auth: {
          ...modules.auth, credentials() { throw new Error("unconfigured local inference must not read credentials"); },
        } },
        network, events: new EventLog(1),
        client: new CorralClient({
          corral: "/not-executed", root: work, identity, hostCap: "h".repeat(64), sessionId: () => "",
        }),
        sessionManager: sdk.SessionManager.create(work, join(work, "sessions")),
      })).rejects.toThrow(/module_invalid/);
    }
  } finally {
    assets.cleanup();
    rmSync(work, { recursive: true, force: true });
  }
});
