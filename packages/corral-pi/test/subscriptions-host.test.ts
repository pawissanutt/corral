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
import { subscriptionRoutes } from "../src/host/subscriptions.ts";
import { CorralClient } from "../src/tools/corral.ts";
import fixtureAuth from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";

for (const role of ["assistant", "pm"] as const) test(`${role}: GPT -> Claude -> GPT preserves history and binding; removed credentials cannot commit`, async () => {
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-subscriptions-"));
  const oldFetch = globalThis.fetch;
  let claudePresent = true;
  const claudePrompts: unknown[] = [];
  const auth: AuthModule = {
    provider: "openai-codex", providers: ["openai-codex", "anthropic"],
    credentials(context) {
      if (context.provider === "openai-codex") return fixtureAuth.credentials(context);
      return {
        read: async (id) => id === "anthropic" && claudePresent
          ? { type: "oauth", access: "sk-ant-oat-SYNTHETIC", refresh: "r", expires: Date.now() + 3600_000 } : undefined,
        list: async () => claudePresent ? [{ providerId: "anthropic", type: "oauth" }] : [],
        modify: async () => { throw new Error("read-only"); }, delete: async () => { claudePresent = false; },
      };
    },
  };
  let host: HostSession | undefined;
  try {
    const modules = { ...await loadModules(assets.dir, "worker"), auth };
    const routes = await subscriptionRoutes(auth, "openai-codex", role);
    expect(routes.providers).toEqual(["openai-codex", "anthropic"]);
    const network = guardedFetch(fixtureAuth.transport!({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: "" }), undefined, routes.providers);
    globalThis.fetch = network.fetch as typeof fetch;
    overrideSdk({
      query: ({ prompt }) => Object.assign((async function* () {
        // One reply per pushed message, as Claude Code's streaming input does; a one-shot prompt ends after its only message.
        for await (const message of prompt) {
          claudePrompts.push(message);
          yield { type: "result", subtype: "success", is_error: false, result: "CLAUDE", stop_reason: "end_turn",
            usage: { input_tokens: 12, output_tokens: 5 }, user_message_uuid: message.uuid };
        }
      })(), { interrupt: async () => undefined, close: () => undefined }) as never,
      createSdkMcpServer: (options) => ({ type: "sdk", name: options.name, instance: options }) as never,
    });
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role };
    const corral = writeFakeEngine(work);
    const events = new EventLog();
    const cwd = join(work, "work"); mkdirSync(cwd);
    host = await HostSession.create({ sdk, identity, cwd, agentDir: join(work, "agent"), model: "gpt-6.1-sol", modules,
      client: new CorralClient({ corral: corral.path, root: work, cwd, identity }),
      events, network, sessionManager: sdk.SessionManager.create(cwd, join(work, "sessions")),
    });
    await host.startLocal();
    const session = host.session;
    const id = host.sessionId();
    const file = session.sessionFile;
    const models = host.runtime.services.modelRuntime;
    const available = await models.getAvailable();
    expect(new Set(available.map((m) => m.provider))).toEqual(new Set(routes.providers));
    const gpt = models.getModel("openai-codex", "gpt-6.1-sol")!;
    const claude = models.getModel("anthropic", "claude-opus-5-5")!;
    await session.prompt("Reply only FIRST.");
    const history = [...session.messages];
    await session.setModel(claude);
    expect(session.messages).toEqual(history);
    await session.prompt("Reply only CLAUDE. Remember FIRST.");
    expect(JSON.stringify(claudePrompts)).toContain("FIRST");
    expect(session.getLastAssistantText()).toBe("CLAUDE");
    await session.setModel(gpt);
    await session.prompt("Reply only BACK.");
    expect(host.session).toBe(session);
    expect(host.sessionId()).toBe(id);
    expect(session.sessionFile).toBe(file);
    expect(session.messages.slice(0, history.length)).toEqual(history);
    await host.drained();
    expect(events.since(0, 64 * 1024).events.filter(event => event.kind === "turn_started").map(event => [event.provider, event.model]))
      .toEqual([["openai-codex", gpt.id], ["anthropic", claude.id], ["openai-codex", gpt.id]]);
    claudePresent = false;
    const leaf = session.sessionManager.getLeafId();
    await expect(session.setModel(claude)).rejects.toThrow("credentials_unavailable");
    expect(session.model).toEqual(gpt);
    expect(session.sessionManager.getLeafId()).toBe(leaf);
    await expect(session.setModel({ ...gpt, provider: "openai" })).rejects.toThrow("model_unavailable");
    await session.prompt("Reply only STILL_WORKING.");
    expect(session.getLastAssistantText()).toContain("STILL_WORKING");
    await expect(network.fetch("https://api.openai.com/v1/responses", { method: "POST" })).rejects.toThrow("network_refused");
    // Claude model requests belong only to its subprocess lane, not the host's fetch.
    await expect(network.fetch("https://api.anthropic.com/v1/messages", { method: "POST" })).rejects.toThrow("network_refused");
    expect(network.destinations).toContain("https://platform.claude.com/v1/oauth/token");
    expect(network.destinations).not.toContain("https://api.openai.com/v1/responses");
    const onlyGpt = await subscriptionRoutes(auth, "openai-codex", role);
    expect(onlyGpt.providers).toEqual(["openai-codex"]);
  } finally {
    await host?.close(); overrideSdk(undefined); globalThis.fetch = oldFetch;
    assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
