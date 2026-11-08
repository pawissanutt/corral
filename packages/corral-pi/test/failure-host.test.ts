// One offline native reporting journey per subscription transport. Variants live in failure-signals.test.ts.
import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AuthModule } from "../src/auth/types.ts";
import { overrideSdk } from "../src/claude/sdk.ts";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { CorralClient } from "../src/tools/corral.ts";
import fixtureAuth from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";

for (const provider of ["openai-codex", "anthropic"] as const) test(`${provider}: native quota evidence reaches the bound host event without raw errors`, async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-failure-"));
  const oldFetch = globalThis.fetch;
  let host: HostSession | undefined;
  let requests = 0;
  const secret = "sk-SYNTHETIC-DO-NOT-REPORT";
  try {
    const auth: AuthModule = { provider: "openai-codex", providers: [provider],
      credentials(context) {
        if (provider === "openai-codex") return fixtureAuth.credentials(context);
        return {
          read: async () => ({ type: "oauth", access: "sk-ant-oat-SYNTHETIC", refresh: "r", expires: Date.now() + 3600_000 }),
          list: async () => [{ providerId: "anthropic", type: "oauth" }],
          modify: async () => { throw new Error("read-only"); }, delete: async () => undefined,
        };
      },
    };
    const network = guardedFetch(async () => {
      requests++;
      return Response.json({ error: { type: "usage_limit_reached", message: `quota exceeded ${secret}`, resets_at: 1_800_000_600 } },
        { status: 429, headers: { "x-request-id": secret, "retry-after": "3" } });
    }, undefined, provider);
    globalThis.fetch = network.fetch as typeof fetch;
    overrideSdk({
      query: () => Object.assign((async function* () {
        yield { type: "rate_limit_event", rate_limit_info: { status: "rejected", rateLimitType: "five_hour", resetsAt: 1_800_000_600 } };
        yield { type: "assistant", error: "rate_limit", request_id: secret,
          message: { content: [{ type: "text", text: secret }], stop_reason: null } };
      })(), { interrupt: async () => undefined, close: () => undefined }) as never,
      createSdkMcpServer: options => ({ type: "sdk", name: options.name, instance: options }) as never,
    });
    const modules = { ...await loadModules(assets.dir, "worker"), auth };
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role: "worker" as const, task: "T-001", run: "R1", generation: 1 };
    const corral = writeFakeCorral(work);
    const events = new EventLog(1);
    const cwd = join(work, "work"); mkdirSync(cwd);
    host = await HostSession.create({ sdk, identity, provider, cwd, agentDir: join(work, "agent"),
      model: provider === "anthropic" ? "claude-sonnet-5-5" : "gpt-6.1-sol", modules,
      client: new CorralClient({ corral: corral.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host!.sessionId() }),
      events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.hello("n".repeat(64), "pane-1", "c".repeat(64));
    await host.session.prompt("Offline failure fixture");
    await host.drained();
    const failures = corral.calls().map(c => c.envelope.payload).filter(p => p.kind === "provider_error");
    expect(failures).toHaveLength(1);
    expect(failures[0]).toMatchObject({ class: "quota", failure: { version: 1,
      connection: provider === "anthropic" ? "anthropic-subscription" : "openai-subscription",
      category: "subscription_quota", scope: "account", reset_at: "2027-01-15T08:10:00.000Z" } });
    expect(JSON.stringify(failures)).not.toContain(secret);
    expect(requests).toBe(provider === "anthropic" ? 0 : 1);
    expect(JSON.stringify(host.session.messages)).not.toContain(secret);
  } finally {
    await host?.close(); overrideSdk(undefined); globalThis.fetch = oldFetch;
    assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 15_000);
