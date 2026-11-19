import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import type { AuthModule } from "../auth/types.ts";
import { Spending } from "../capabilities/budget.ts";
import { EventLog } from "../bridge/events.ts";
import { parseBuildFlags } from "../build/launch.ts";
import { createSession, saveSession, sessionDir } from "../build/session-store.ts";
import type { BuildContext } from "../build/types.ts";
import { CorralClient } from "../tools/corral.ts";
import fixtureAuth from "../../test/fixtures/auth/index.ts";
import { fakeCodexTransport } from "../../test/fixtures/auth/fake-transport.ts";
import { writeFakeEngine } from "../../test/support/fake-engine.ts";
import { buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { parseRunFlags } from "./config.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch } from "./restrict.ts";
import { HostSession } from "./session.ts";

async function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "corral-spending-"));
  const assets = buildFixtureInstall({ parent: dir });
  const engine = writeFakeEngine(dir, {});
  const api = fakeCodexTransport(), subscription = fakeCodexTransport();
  let host: HostSession | undefined;
  const key = "sk-SYNTHETIC-fixture-spending";
  const auth: AuthModule = {
    provider: "openai-codex", providers: ["openai-codex", "openai"], connections: ["openai-subscription", "openai-api"],
    credentials(context) {
      if (context.connection !== "openai-api") return fixtureAuth.credentials(context);
      return { read: async () => ({ type: "api_key", key }),
        list: async () => [{ type: "api_key", providerId: "openai" }],
        modify: async () => { throw new Error("read-only"); }, delete: async () => undefined };
    },
    transport(context) {
      if (context.connection !== "openai-api") return subscription.send;
      return (input, init) => {
        expect(new Headers(init?.headers).get("authorization")).toBe(`Bearer ${key}`);
        return api.send(input, init);
      };
    },
  };
  const network = guardedFetch(subscription.send, undefined, "openai-codex");
  const start = async (flags: string[], transcript?: string, build?: BuildContext) => {
    const config = parseRunFlags("run", ["--dir", dir, "--corral", engine.path, ...flags], assets.dir,
      { CORRAL_NEXT_ROOT: dir, CORRAL_TASK: "T-1", CORRAL_RUN: "R1" }, dir);
    const events = new EventLog();
    const identity = build ? { store: "build", project: "", role: "build" as const } : config.identity;
    host = await HostSession.create({ sdk, identity, build, cwd: dir, root: dir, provider: config.provider, model: config.model,
      connection: config.connection, effort: config.effort, sessionDir: build?.dir ?? dir, agentDir: join(dir, "agent"), ui: "plain", events, network,
      spending: Spending.open(build?.dir ?? dir, config), modules: { ...await loadModules(assets.dir, identity.role), auth },
      client: new CorralClient({ ...config, identity, sessionDir: build?.dir ?? dir }),
      sessionManager: transcript ? sdk.SessionManager.open(transcript) : sdk.SessionManager.create(dir, config.sessionDir) });
    await host.startLocal();
    return { host, events };
  };
  return { dir, assets: assets.dir, engine, api, subscription, start, close: async () => { await host?.close(); host = undefined; },
    async clean() { await host?.close(); assets.cleanup(); rmSync(dir, { recursive: true, force: true }); } };
}

test("a priced request uses the local cap; the next request and restart cannot spend again", async () => {
  const f = await fixture();
  try {
    const model = ["--model", "openai/gpt-5.4"];
    const first = await f.start([...model, "--max-usd", "0.01"]);
    f.api.script([{ text: "Priced reply", usage: { input: 1000, output: 2000 } }]);
    await first.host.session.prompt("first request");
    expect(first.host.session.getLastAssistantText()).toBe("Priced reply");
    await first.host.session.prompt("second request");
    expect(first.host.state().last_error).toContain("spending cap reached");
    const transcript = first.host.session.sessionFile!;
    await first.host.drained();
    const path = join(f.dir, "spend.json");
    const saved = readFileSync(path, "utf8");
    expect(JSON.parse(saved).spentUsd).toBeGreaterThanOrEqual(0.01);
    await f.close();
    const resumed = await f.start(model, transcript);
    await resumed.host.session.prompt("restart does not replenish");
    expect(resumed.host.state().last_error).toContain("spending cap reached");
    expect(readFileSync(path, "utf8")).toBe(saved);
    expect(f.api.requests).toBe(1);
    expect(() => Spending.open(f.dir, { maxUsd: 1 })).toThrow("Cannot change --max-usd");
    const usage = f.engine.calls().filter(argv => argv.includes("usage"));
    const usd = String(JSON.parse(saved).spentUsd);
    expect(usage).toEqual([["--root", f.dir, "--json", "usage", "add", "--model", "openai/gpt-5.4", "--task", "T-1", "--run", "R1",
      "--input", "1000", "--output", "2000", "--cache-read", "0", "--cache-write", "0", "--usd", usd, "--billing", "api"]]);
  } finally { await f.clean(); }
});

test("subscription quota continues in the same conversation, with the API bill persisted", async () => {
  const f = await fixture();
  try {
    const { host, events } = await f.start(["--max-usd", "0.01", "--paid-route", "openai/gpt-5.4"]);
    f.subscription.script([{ text: "Subscription reply" },
      { error: { status: 429, body: { error: { type: "usage_limit_reached" } } } }]);
    f.api.script([{ text: "Paid reply" }, { text: "Still paid" }]);
    await host.session.prompt("Subscription turn");
    const transcript = host.session.sessionFile;
    await host.session.prompt("Continue after quota");
    expect(host.session.getLastAssistantText()).toBe("Paid reply");
    expect(host.session.sessionFile).toBe(transcript);
    expect(f.api.prompts).toEqual(["Continue after quota"]);
    expect(host.session.sessionManager.getBranch().filter(entry => entry.type === "custom_message" && entry.customType === "corral_paid_continuation")).toHaveLength(1);
    await host.session.prompt("Next paid turn");
    expect(host.session.getLastAssistantText()).toBe("Still paid");
    expect(f.subscription.requests).toBe(2);
    expect(events.since(0, 65536).events.filter(event => event.kind === "paid_continuation")).toHaveLength(1);
    const saved = JSON.parse(readFileSync(join(f.dir, "spend.json"), "utf8"));
    expect(saved).toMatchObject({ maxUsd: 0.01, paidRoute: "openai/gpt-5.4" });
    expect(saved.spentUsd).toBeGreaterThan(0);
    await host.drained();
    expect(f.engine.calls().filter(argv => argv.at(-1) === "api")).toHaveLength(2);
  } finally { await f.clean(); }
});

test("named API Build children on different models spend the same cap as their subscription parent", async () => {
  const f = await fixture();
  try {
    const config = join(f.dir, ".corral-next");
    mkdirSync(config);
    writeFileSync(join(config, "corral.toml"), `[profiles.mixed]\nharness = "pi"\nmodel = "openai-codex/gpt-6.1-sol"\nargs = ["--max-usd", "0.01"]\n`);
    writeFileSync(join(config, "pi.toml"), `[build.profiles.first]\nmodel = "openai/openai/gpt-5.4"\n[build.profiles.second]\nmodel = "openai/gpt-5.2"\n`);
    const { launch } = parseBuildFlags(["--profile", "mixed", "--data-dir", f.dir, "--here"], f.assets, { CORRAL_NEXT_ROOT: f.dir }, f.dir);
    expect(launch.maxUsd).toBe(0.01);
    const record = { ...createSession(launch, { worktree: f.dir, branch: null, base: null }),
      workspaceRoot: undefined, project: undefined, bare: true, repository: false };
    saveSession(f.dir, record);
    const build: BuildContext = { session: record, settings: launch.settings, dir: sessionDir(f.dir, record.id), corral: f.engine.path };
    const { host } = await f.start(["--model", launch.model, "--max-usd", String(launch.maxUsd)], undefined, build);
    f.api.script([{ text: "Ran first child", usage: { input: 1000, output: 2000 } }]);
    f.subscription.script([
      { toolCalls: [{ name: "task", callId: "call_task", arguments: { agents: [
        { id: "first", profile: "first", title: "First", brief: "First metered child", owns: [] },
        { id: "second", profile: "second", title: "Second", brief: "Second metered child", owns: [], after: ["first"] },
      ] } }] },
      { toolCalls: [{ name: "agent_wait", callId: "call_wait", arguments: { ids: ["first", "second"] } }] },
      { text: "Parent stays on subscription" },
    ]);
    await host.session.prompt("Start both named children");
    await host.drained();
    const journal = JSON.parse(readFileSync(join(build.dir, "journal.json"), "utf8"));
    expect(journal.children).toMatchObject([
      { id: "first", model: "openai/gpt-5.4", state: "done" },
      { id: "second", model: "openai/gpt-5.2", state: "failed" },
    ]);
    expect(journal.children[1]?.error).toContain("spending cap reached");
    expect(f.api.requests).toBe(1);
    expect(host.session.getLastAssistantText()).toBe("Parent stays on subscription");
    expect(JSON.parse(readFileSync(join(build.dir, "spend.json"), "utf8")).spentUsd).toBeGreaterThanOrEqual(0.01);
  } finally { await f.clean(); }
});
