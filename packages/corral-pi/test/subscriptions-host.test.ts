import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { FileCredentialStore } from "../src/auth/store.ts";
import type { AuthModule } from "../src/auth/types.ts";
import { overrideSdk } from "../src/claude/sdk.ts";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { sessionRoutes } from "../src/host/capabilities.ts";
import { restoreModelChoice } from "../src/host/lifecycle.ts";
import { CorralClient } from "../src/tools/corral.ts";
import fixtureAuth from "./fixtures/auth/index.ts";
import { fakeCodexTransport } from "./fixtures/auth/fake-transport.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";

test("assistant switches subscription -> Anthropic API -> subscription with history and isolated credentials", async () => {
  const role = "assistant";
  const assets = buildFixtureInstall();
  const work = mkdtempSync(join(tmpdir(), "corral-subscriptions-"));
  const oldFetch = globalThis.fetch;
  const key = "sk-SYNTHETIC-anthropic-api-key", oauth = "sk-ant-oat-SYNTHETIC";
  const apiStore = new FileCredentialStore(join(work, "auth"), "anthropic-api");
  const api = fakeCodexTransport();
  api.script([{ text: "API", usage: { input: 12, output: 5 } }]);
  const ambient = process.env.ANTHROPIC_API_KEY;
  process.env.ANTHROPIC_API_KEY = key;
  let claudePresent = true;
  const claudePrompts: unknown[] = [];
  const auth: AuthModule = {
    provider: "openai-codex", providers: ["openai-codex", "anthropic", "anthropic-api"],
    credentials(context) {
      if (context.provider === "openai-codex") return fixtureAuth.credentials(context);
      if (context.provider === "anthropic-api") return apiStore;
      return {
        read: async (id) => id === "anthropic" && claudePresent
          ? { type: "oauth", access: oauth, refresh: "r", expires: Date.now() + 3600_000 } : undefined,
        list: async () => claudePresent ? [{ providerId: "anthropic", type: "oauth" }] : [],
        modify: async () => { throw new Error("read-only"); }, delete: async () => { claudePresent = false; },
      };
    },
    transport(context) {
      if (context.provider !== "anthropic-api") return fixtureAuth.transport!(context);
      return (input, init) => {
        const url = new URL(input instanceof Request ? input.url : input);
        expect(url.origin + url.pathname).toBe("https://api.anthropic.com/v1/messages");
        const headers = new Headers(init?.headers);
        expect(headers.get("x-api-key")).toBe(key);
        expect(headers.has("authorization")).toBe(false);
        return api.send(input, init);
      };
    },
  };
  let host: HostSession | undefined;
  try {
    await apiStore.modify("anthropic-api", async () => ({ type: "api_key", key }));
    const modules = { ...await loadModules(assets.dir, "assistant"), auth };
    const routes = await sessionRoutes(auth, "openai-codex", role);
    expect(routes.providers).toEqual(["openai-codex", "anthropic", "anthropic-api"]);
    const network = guardedFetch(fixtureAuth.transport!({ provider: "openai-codex", apiEndpoint: "", refreshEndpoint: "" }),
      undefined, ["openai-codex", "anthropic"]);
    globalThis.fetch = network.fetch as typeof fetch;
    overrideSdk({
      query: ({ prompt, options }) => {
        expect(options.env?.CLAUDE_CODE_OAUTH_TOKEN).toBe(oauth);
        expect(JSON.stringify(options.env)).not.toContain(key);
        return Object.assign((async function* () {
          // One reply per pushed message, as Claude Code's streaming input does.
          for await (const message of prompt) {
            claudePrompts.push(message);
            yield { type: "result", subtype: "success", is_error: false, result: "CLAUDE", stop_reason: "end_turn",
              usage: { input_tokens: 12, output_tokens: 5 }, user_message_uuid: message.uuid };
          }
        })(), { interrupt: async () => undefined, close: () => undefined }) as never;
      },
      createSdkMcpServer: (options) => ({ type: "sdk", name: options.name, instance: options }) as never,
    });
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-0123456789abcdef", project: "demo", role } as const;
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
    const apiModel = models.getModel("anthropic-api", "claude-opus-5-5")!;
    await session.cycleModel();
    expect(session.model?.provider).not.toBe("anthropic-api");
    await session.setModel(gpt);
    await session.prompt("Reply only FIRST.");
    const history = [...session.messages];
    await session.setModel(claude);
    expect(session.messages).toEqual(history);
    await session.prompt("Reply only CLAUDE. Remember FIRST.");
    expect(JSON.stringify(claudePrompts)).toContain("FIRST");
    expect(session.getLastAssistantText()).toBe("CLAUDE");
    await session.setModel(apiModel);
    const reopened = sdk.SessionManager.open(file!);
    expect(restoreModelChoice(reopened, true)).toEqual({
      provider: "anthropic-api", model: apiModel.id,
    });
    await session.cycleModel();
    expect(session.model?.provider).toBe("anthropic-api");
    await session.setModel(apiModel);
    await session.prompt("Reply only API.");
    expect(session.getLastAssistantText()).toBe("API");
    expect(api.requests).toBe(1);
    await session.setModel(gpt);
    await session.prompt("Reply only BACK.");
    expect(host.session).toBe(session);
    expect(host.sessionId()).toBe(id);
    expect(session.sessionFile).toBe(file);
    expect(session.messages.slice(0, history.length)).toEqual(history);
    await host.drained();
    expect(events.since(0, 64 * 1024).events.filter(event => event.kind === "turn_started").map(event => [event.provider, event.model]))
      .toEqual([["openai-codex", gpt.id], ["anthropic", claude.id], ["anthropic-api", apiModel.id], ["openai-codex", gpt.id]]);
    const reports = corral.calls().filter(args => args.includes("usage"));
    expect(reports.map(args => [args[args.indexOf("--model") + 1], args.at(-1)])).toEqual([
      [`openai-codex/${gpt.id}`, "subscription"], [`anthropic/${claude.id}`, "subscription"],
      [`anthropic-api/${apiModel.id}`, "api"], [`openai-codex/${gpt.id}`, "subscription"],
    ]);
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
    const withoutClaude = await sessionRoutes(auth, "openai-codex", role);
    expect(withoutClaude.providers).toEqual(["openai-codex", "anthropic-api"]);
  } finally {
    await host?.close(); overrideSdk(undefined); globalThis.fetch = oldFetch;
    if (ambient === undefined) delete process.env.ANTHROPIC_API_KEY;
    else process.env.ANTHROPIC_API_KEY = ambient;
    assets.cleanup(); rmSync(work, { recursive: true, force: true });
  }
}, 60_000);
