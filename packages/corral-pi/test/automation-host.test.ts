import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../src/bridge/events.ts";
import { attachHeadless } from "../src/host/headless.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { authContext, HostSession } from "../src/host/session.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";

test("only the accepted automation input gains the live policy, without persisting it", async () => {
  const assets = buildFixtureInstall();
  const cwd = mkdtempSync(join(tmpdir(), "corral-policy-"));
  const originalFetch = globalThis.fetch;
  let host: HostSession | undefined;
  try {
    restrictEnvironment(process.env);
    const modules = await loadModules(assets.dir, "assistant");
    const send = modules.auth.transport?.(authContext());
    if (!send) throw new Error("fixture transport missing");
    const requests: string[] = [];
    const network = guardedFetch(send);
    globalThis.fetch = network.fetch;
    const sdk = await import("@earendil-works/pi-coding-agent");
    const identity = { store: "st-demo", project: "demo", role: "assistant" as const, assistant: "auto" };
    const events = new EventLog();
    const instruction = "SAVED_RULE_POLICY";
    const sessionManager = sdk.SessionManager.create(cwd, join(cwd, "sessions"));
    host = await HostSession.create({
      sdk, modules, identity, events, network, cwd, agentDir: join(cwd, "agent"), model: "gpt-6.1-sol",
      sessionManager, automationRoot: cwd, automation: ["triage", instruction],
      client: new CorralClient({ corral: writeFakeEngine(cwd).path, root: cwd, cwd, identity }),
    });
    const agent = host.session.agent;
    const stream = agent.streamFunction;
    agent.streamFunction = (model, context, options) => {
      requests.push(JSON.stringify(context.messages));
      return stream(model, context, options);
    };
    await host.startLocal();
    await attachHeadless(host, events, () => undefined);
    const trigger = { text: "/plan event data", origin: "auto" as const, busyPolicy: "steer" as const };
    const permits = join(cwd, ".corral-next", "automation");
    mkdirSync(permits, { recursive: true });
    const issue = (nonce: string) => {
      const path = join(permits, `.admit-${nonce}`);
      writeFileSync(path, "", { flag: "wx" });
      return path;
    };
    const expired = "1".repeat(32);
    const revoked = issue(expired);
    const reached = Promise.withResolvers<void>(), release = Promise.withResolvers<void>();
    // Hold after Pi's initial admission checks and async run preparation, immediately before the actual start.
    const session = host.session as unknown as { _prepareRun(...args: unknown[]): Promise<unknown> };
    const prepare = session._prepareRun;
    session._prepareRun = async (...args) => {
      const messages = await prepare.apply(session, args);
      reached.resolve();
      await release.promise;
      return messages;
    };
    const late = host.submitAutomation(trigger, "auto-expired", expired);
    await reached.promise;
    unlinkSync(revoked); // daemon timeout or the next workspace fence revokes the outstanding grant
    release.resolve();
    expect(await late).toEqual({ disposition: "rejected", reason: "automation_permit_missing" });
    expect(requests).toHaveLength(0);
    session._prepareRun = prepare;
    const nonce = "2".repeat(32);
    const permit = issue(nonce);
    expect(await host.submitAutomation(trigger, "auto-1", nonce)).toMatchObject({ disposition: "started" });
    expect(existsSync(permit)).toBe(false);
    await host.session.waitForIdle();
    expect(await host.submitOpened({ ...trigger, text: "Ordinary auto notice" })).toMatchObject({ disposition: "started" });
    await host.session.waitForIdle();
    expect(requests).toHaveLength(2);
    expect(requests[0]).toContain(instruction);
    expect(requests[1]).not.toContain(instruction);
    expect(JSON.stringify(sessionManager.getEntries())).not.toContain(instruction);
    expect(host.retireAutomation()).toEqual({ retired: true });
    expect(await host.submitAutomation(trigger, "auto-2", nonce)).toEqual({ disposition: "rejected", reason: "retired" });
  } finally {
    await host?.close();
    await host?.drained();
    globalThis.fetch = originalFetch;
    assets.cleanup();
    rmSync(cwd, { recursive: true, force: true });
  }
});
