// gpt-6.1-terra resolves through a real HostSession on the fixture transport with only the bundled
// catalog (no owner models.json), carries its long-context price tier, and an unknown model is
// refused with model_unavailable before the transport is used.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { calculateCost } from "@earendil-works/pi-ai";

import { EventLog } from "../bridge/events.ts";
import { CorralClient, sha256Hex } from "../tools/corral.ts";
import { writeFakeCorral } from "../../test/support/fake-corral.ts";
import { type FixtureInstall, buildFixtureInstall } from "../../test/support/fixture-install.ts";
import { loadModules } from "./modules.ts";
import { guardedFetch, restrictEnvironment } from "./restrict.ts";
import { HostSession, authContext } from "./session.ts";
import { SUPPLEMENTAL_MODELS } from "./supplemental-models.ts";

const originalFetch = globalThis.fetch;
const previousXdg = process.env.XDG_DATA_HOME;
const identity = {
  store: "st-0123456789abcdef", project: "demo", role: "worker", generation: 1, task: "T-001", run: "R1",
} as ConstructorParameters<typeof CorralClient>[0]["identity"];

let assets: FixtureInstall;
let work: string;

beforeAll(() => {
  assets = buildFixtureInstall({ omit: ["delivery"] });
  work = mkdtempSync(join(tmpdir(), "corral-pi-terra-"));
  process.env.XDG_DATA_HOME = join(work, "xdg");
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
  else process.env.XDG_DATA_HOME = previousXdg;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

async function start(model: string, effort?: string): Promise<HostSession> {
  const modules = await loadModules(assets.dir, "worker");
  const dir = join(work, model.replace(/\W/g, "_"));
  mkdirSync(join(dir, "worktree"), { recursive: true });
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("the fixture auth module supplies a transport");
  const network = guardedFetch(transport, () => undefined);
  globalThis.fetch = network.fetch as typeof globalThis.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const corral = writeFakeCorral(dir);
  let host: HostSession | undefined;
  const client = new CorralClient({ corral: corral.path, root: dir, identity, hostCap: "h".repeat(64), sessionId: () => host!.sessionId() });
  host = await HostSession.create({
    sdk, identity, cwd: join(dir, "worktree"), agentDir: join(dir, "pi-agent"), model, ...(effort ? { effort } : {}),
    modules, client, events: new EventLog(1), network,
    sessionManager: sdk.SessionManager.create(join(dir, "worktree"), join(dir, "sessions")),
  });
  await host.hello("n".repeat(64), "pane-worker", sha256Hex("c".repeat(64)));
  return host;
}

test("a host session starts on openai-codex/gpt-6.1-terra at medium effort without an owner models.json", async () => {
  const host = await start("openai-codex/gpt-6.1-terra", "medium");
  expect(host.state().model).toBe("openai-codex/gpt-6.1-terra");
  expect(host.session.model?.id).toBe("gpt-6.1-terra");
  expect(host.session.thinkingLevel).toBe("medium");
}, 60_000);

test("a model absent from every catalog is refused with model_unavailable", async () => {
  await expect(start("openai-codex/gpt-9-missing", "medium")).rejects.toThrow("model_unavailable: openai-codex/gpt-9-missing");
}, 60_000);

test("Terra prices usage below and above the 272000-token long-context threshold", () => {
  const terra = SUPPLEMENTAL_MODELS["openai-codex"]![0]!;
  const usage = (input: number) => ({
    input, output: 1000, cacheRead: 0, cacheWrite: 0, totalTokens: input + 1000,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  });
  expect(Math.abs(calculateCost(terra, usage(100000)).total - 0.105) < 1e-6).toBe(true);
  expect(Math.abs(calculateCost(terra, usage(300000)).total - 0.6075) < 1e-6).toBe(true);
});
