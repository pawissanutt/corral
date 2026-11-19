import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { AuthModule } from "../auth/types.ts";
import { EventLog } from "../bridge/events.ts";
import { CorralClient } from "../tools/corral.ts";
import fixtureAuth from "../../test/fixtures/auth/index.ts";
import { fakeCodexTransport } from "../../test/fixtures/auth/fake-transport.ts";
import { writeFakeEngine } from "../../test/support/fake-engine.ts";
import { buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { parseRunFlags } from "./config.ts";
import { restoreModelChoice } from "./lifecycle.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch } from "./restrict.ts";
import { HostSession } from "./session.ts";

function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "corral-model-fallback-"));
  const assets = buildFixtureInstall({ parent: dir });
  const engine = writeFakeEngine(dir, {});
  const api = fakeCodexTransport(), subscription = fakeCodexTransport();
  let host: HostSession | undefined;
  const key = "sk-SYNTHETIC-fixture-fallback";
  const auth: AuthModule = {
    provider: "openai-codex", providers: ["openai-codex", "openai"],
    credentials(context) {
      if (context.provider !== "openai") return fixtureAuth.credentials(context);
      return { read: async () => ({ type: "api_key", key }),
        list: async () => [{ type: "api_key", providerId: "openai" }],
        modify: async () => { throw new Error("read-only"); }, delete: async () => undefined };
    },
    transport(context) {
      if (context.provider !== "openai") return subscription.send;
      return (input, init) => {
        expect(new Headers(init?.headers).get("authorization")).toBe(`Bearer ${key}`);
        return api.send(input, init);
      };
    },
  };
  const network = guardedFetch(subscription.send, undefined, "openai-codex");
  const start = async (command: "run" | "chat", flags: string[]) => {
    const config = parseRunFlags(command, ["--dir", dir, "--corral", engine.path, ...flags], assets.dir,
      { CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-1", CORRAL_RUN: "R1" }, dir);
    const events = new EventLog();
    host = await HostSession.create({ sdk, identity: config.identity, cwd: dir, model: config.model, provider: config.provider,
      fallback: config.fallback, agentDir: join(dir, "agent"), ui: "plain", events, network,
      modules: { ...await loadModules(assets.dir, config.identity.role), auth }, client: new CorralClient(config),
      sessionManager: sdk.SessionManager.create(dir, config.sessionDir) });
    await host.startLocal();
    return { host, events };
  };
  return { engine, api, subscription, start,
    async clean() { await host?.close(); assets.cleanup(); rmSync(dir, { recursive: true, force: true }); } };
}

const quota = { error: { status: 429, body: { error: { type: "usage_limit_reached" } } } };

test("worker quota skips unusable entries, selects the next model and answers within the same turn", async () => {
  const f = fixture();
  try {
    const { host, events } = await f.start("run", ["--fallback", "anthropic-api/claude-opus-5-5",
      "--fallback", "openai/not-a-model", "--fallback", "openai/gpt-5.4"]);
    const session = host.session, transcript = session.sessionFile!;
    await expect(host.selectRemoteModel("openai/gpt-5.4")).rejects.toThrow("model_unavailable");
    f.subscription.script([quota]);
    f.api.script([{ text: "Metered reply" }, { text: "Still on fallback" }]);
    await session.prompt("Continue after quota");
    expect(session.getLastAssistantText()).toBe("Metered reply");
    expect(session.sessionFile).toBe(transcript);
    expect(f.api.prompts).toEqual(["Continue after quota"]);
    expect(host.state().model).toBe("openai/gpt-5.4");
    const observed = events.since(0, 65536).events;
    expect(observed.filter(event => event.kind === "turn_started")).toHaveLength(1);
    expect(observed.filter(event => event.kind === "model_fallback")).toHaveLength(1);
    expect(session.sessionManager.getBranch().filter(entry => entry.type === "custom_message" && entry.customType === "corral_model_fallback")).toHaveLength(1);
    const paid = { provider: "openai", model: "gpt-5.4" } as const;
    expect(restoreModelChoice(sdk.SessionManager.open(transcript), false, [paid])).toEqual(paid);
    await session.prompt("Next turn");
    expect(session.getLastAssistantText()).toBe("Still on fallback");
    expect(f.subscription.requests).toBe(1);
    await host.drained();
    expect(f.engine.calls().filter(argv => argv.at(-1) === "api")).toHaveLength(2);
  } finally { await f.clean(); }
});

test("quota without fallback surfaces even when a metered key is available", async () => {
  const f = fixture();
  try {
    const { host, events } = await f.start("chat", []);
    expect(await host.session.modelRuntime.checkAuth("openai")).toBeDefined();
    f.subscription.script([quota]);
    await host.session.prompt("Unused API key");
    expect(f.api.requests).toBe(0);
    expect(host.state().model).toBe("openai-codex/gpt-6.1-sol");
    expect(events.since(0, 65536).events.some(event => event.kind === "model_fallback")).toBe(false);
    expect(host.state().last_error).toContain("subscription_quota");
  } finally { await f.clean(); }
});
