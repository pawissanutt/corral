import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import * as sdk from "@earendil-works/pi-coding-agent";
import { connectionRules } from "../src/auth/connections.ts";
import { EventLog } from "../src/bridge/events.ts";
import { LocalControlPlane } from "../src/build/control-plane.ts";
import { createBuildBudgetRecord } from "../src/build/paid.ts";
import { createSession, saveSession, sessionDir } from "../src/build/session-store.ts";
import type { BuildContext, BuildLaunch } from "../src/build/types.ts";
import { buildHostConfig } from "../src/host/build.ts";
import { bindingDigest, type ConnectionBinding } from "../src/host/connections.ts";
import { loadModules } from "../src/host/modules.ts";
import { guardedFetch } from "../src/host/restrict.ts";
import { HostSession } from "../src/host/session.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";

test("paid Build hello authenticates its Rust ledger without a bound-role continuation notice", async () => {
  const assets = buildFixtureInstall();
  const root = mkdtempSync(join(tmpdir(), "corral-build-paid-hello-"));
  let host: HostSession | undefined;
  try {
    const cwd = join(root, "work"); mkdirSync(cwd);
    const git = (...args: string[]) => execFileSync("git", ["-C", cwd, ...args], { encoding: "utf8", stdio: "pipe" }).trim();
    git("init", "--initial-branch=main");
    git("-c", "user.name=Build Test", "-c", "user.email=build@example.test", "commit", "--allow-empty", "-m", "base");
    const target = { version: 1 as const, connection: "anthropic-api" as const, identity: "anthropic-api", origin: "registered" as const,
      candidate: "exact-api", model: "claude-sonnet-4-6", effort: null };
    const binding: ConnectionBinding = { ...target, digest: bindingDigest(target) };
    const launch: BuildLaunch = { version: 1, assets: assets.dir, dataRoot: root, corral: "/synthetic/corral", workspaceRoot: root,
      repoRoot: cwd, here: true, provider: "anthropic", model: binding.model, binding,
      paid: { session: "B-fixture1", generation: 3, cap: "c".repeat(64) },
      settings: { exitPolicy: "none", maxParallel: 1, profiles: [] } };
    const record = createSession(launch, { worktree: cwd, branch: "main", base: git("rev-parse", "HEAD") });
    saveSession(root, record);
    const config = buildHostConfig(launch, record, assets.dir);
    const authority = { admitted: true, exhausted: false, connection: binding.connection, binding_digest: binding.digest,
      paid: { episode: record.id, epoch: launch.paid!.generation, allowance_nanos: 50_000_000_000 },
      limits: { max_tokens: null, max_usd_nanos: 50_000_000_000 } };
    let reply: Record<string, unknown> = authority;
    const actions: string[] = [];
    const paidRecord = createBuildBudgetRecord(launch, async (_argv, options) => {
      actions.push(JSON.parse(options.input!).action);
      return { code: 0, stdout: JSON.stringify(reply), stderr: "" };
    });
    if (!paidRecord) throw new Error("fixture requires a paid Build launch");
    const build: BuildContext = { session: record, settings: launch.settings, dir: sessionDir(root, record.id),
      corral: launch.corral!, paidRecord };
    // The temporary installation owns private fixture state; a static import would share it across tests.
    const fixtureAuth = await import(join(assets.dir, "src/auth/index.ts"));
    fixtureAuth.enablePaidFixture();
    const modules = await loadModules(assets.dir, "build", "anthropic");
    let networkRequests = 0;
    const network = guardedFetch(async () => { networkRequests++; throw new Error("provider requests are forbidden in startup regression"); },
      undefined, ["anthropic"], connectionRules("anthropic-api"));
    const client = new LocalControlPlane({ dir: build.dir, root, identity: config.identity, sessionId: () => host?.sessionId() ?? "" });
    host = await HostSession.create({ sdk, identity: config.identity, build, cwd, agentDir: join(root, "agent"),
      provider: "anthropic", model: binding.model, binding, root, runtimeDir: config.runtimeDir,
      capabilities: config.capabilities, modules, client, network, events: new EventLog(1),
      sessionManager: sdk.SessionManager.create(cwd, config.sessionDir) });
    for (const invalid of [
      { admitted: true, exhausted: false, limits: { max_tokens: null, max_usd_nanos: null } },
      { ...authority, binding_digest: "0".repeat(64) },
      { ...authority, paid: { ...authority.paid, episode: "B-another1" } },
    ]) {
      reply = invalid;
      await expect(host.hello("nonce", "", "ctl")).rejects.toThrow("paid_admission_required");
      expect(host.bound).toBeFalse();
    }
    reply = authority;
    await host.hello("nonce", "", "ctl");
    expect(host.bound).toBeTrue();
    expect(actions.length).toBeGreaterThan(0);
    expect(actions.every(action => action === "state")).toBeTrue();
    expect(networkRequests).toBe(0);
  } finally {
    await host?.close();
    assets.cleanup();
    rmSync(root, { recursive: true, force: true });
  }
});
