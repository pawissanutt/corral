import { expect, test } from "bun:test";
import { normalizeContext, type Api, type Model } from "@earendil-works/pi-ai";
import { RunBudget } from "../capabilities/budget.ts";
import { bindingDigest, type ConnectionBinding } from "../host/connections.ts";
import { createBuildBudgetRecord } from "./paid.ts";
import { selectBuildProfile } from "./profiles.ts";
import type { BuildContext, BuildLaunch, BuildProfileRoute } from "./types.ts";
import type { Run } from "./corral-cli.ts";
import { createSession } from "./session-store.ts";
import { mkdtempSync, openSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readBuildLaunch } from "./launch.ts";
import { resolveModel } from "../extensions/fleet/profiles.ts";

const model: Model<Api> = { id: "gpt-5.4", provider: "openai", api: "openai-responses", baseUrl: "https://api.openai.com/v1",
  name: "fixture", input: ["text"], reasoning: false, contextWindow: 20, maxTokens: 5,
  cost: { input: 1, output: 1, cacheRead: 1, cacheWrite: 0.5 } };
const base = { version: 1 as const, connection: "openai-api" as const, identity: "openai-api", origin: "registered" as const,
  candidate: "exact-api", model: model.id, effort: null };
const binding: ConnectionBinding = { ...base, digest: bindingDigest(base) };
const route: BuildProfileRoute = { name: "api", label: "API", provider: "openai", model: model.id, binding,
  paid: { policy_digest: "11".repeat(32), approval: "fixture", allowance_nanos: 50_000_000_000, candidates: [binding] } };
const launch: BuildLaunch = { version: 1, assets: "/fixture/assets", dataRoot: "/fixture/data", corral: "/fixture/corral", workspaceRoot: "/fixture/workspace",
  repoRoot: "/fixture/repo", here: false, provider: "openai", model: model.id, binding, route,
  paid: { session: "B-fixture1", generation: 3, cap: "22".repeat(32) }, settings: { exitPolicy: "none", maxParallel: 3, profiles: [] } };

test("Build parent and child physical attempts use one Rust capability, never local unlimited accounting", async () => {
  const calls: Record<string, unknown>[] = [];
  const run: Run = async (argv, options) => {
    expect(argv).toEqual(["--root", launch.workspaceRoot!, "build", "budget"]);
    expect(argv.join(" ")).not.toContain(launch.paid!.cap);
    const call = JSON.parse(options.input!);
    calls.push(call);
    expect(call.session).toBe(launch.paid!.session); expect(call.generation).toBe(3); expect(call.cap).toBe(launch.paid!.cap);
    return { code: 0, stderr: "", stdout: JSON.stringify({ admitted: calls.length === 1, exhausted: false,
      connection: binding.connection, binding_digest: binding.digest, paid: { episode: launch.paid!.session, epoch: 3 } }) };
  };
  const record = createBuildBudgetRecord(launch, run)!;
  let stopped = 0;
  const budget = new RunBudget({ record, cancel() { stopped++; } });
  const request = { binding, model, context: normalizeContext({ messages: [] }), options: { maxTokens: 5 } };
  await budget.physical(request); // uncertain parent attempt
  await expect(budget.physical(request)).rejects.toThrow("budget_exhausted"); // child/retry shares authority
  expect(calls).toHaveLength(2); expect(calls[0]!.request).not.toBe(calls[1]!.request);
  expect(stopped).toBe(1);
  await expect(createBuildBudgetRecord(launch, async () => ({ code: 1, stderr: launch.paid!.cap, stdout: "" }))!({ kind: "budget", action: "state" }))
    .rejects.toThrow("Build financial authority refused");
});

test("Build paid profile previews are sticky and cannot mutate the saved route", async () => {
  const context: BuildContext = { session: createSession(launch, { worktree: "/fixture", branch: null, base: null }), settings: launch.settings,
    profileCatalog: [route, { name: "sub", label: "Subscription", provider: "openai-codex", model: model.id }], dir: "/fixture", corral: "corral" };
  expect((await selectBuildProfile(context, "api")).binding?.digest).toBe(binding.digest);
  await expect(selectBuildProfile(context, "sub")).rejects.toThrow("sticky");
  expect(context.session.route).toBe(route);
});

test("a subscription Build admits named API children without changing the parent route", async () => {
  const secondBase = { ...base, candidate: "other-api", model: "openai/other-model" };
  const second = { ...secondBase, digest: bindingDigest(secondBase) };
  const profiles = [binding, second].map((route, index) => ({ name: `child-${index}`, model: `openai/${route.model}`, binding: route }));
  const wire = { version: 1, assets: launch.assets, data_root: launch.dataRoot, corral: launch.corral, repo_root: launch.repoRoot,
    workspace_root: launch.workspaceRoot, here: false, provider: "openai-codex", model: "subscription-model",
    settings: { exit_policy: "none", max_parallel: 3, profiles } };
  const dir = mkdtempSync(join(tmpdir(), "corral-build-mixed-launch-"));
  const file = join(dir, "launch.json");
  try {
    writeFileSync(file, JSON.stringify(wire));
    expect(() => readBuildLaunch(openSync(file, "r"))).toThrow();
    writeFileSync(file, JSON.stringify({ ...wire, paid: { ...launch.paid, mixed: true } }));
    const parsed = readBuildLaunch(openSync(file, "r"));
    expect(parsed.provider).toBe("openai-codex");
    expect(resolveModel("child-0", parsed.settings.profiles, ["openai-codex"]).binding).toEqual(binding);
    expect(resolveModel("child-1", parsed.settings.profiles, ["openai-codex"])).toMatchObject({ model: "other-model", binding: second });
    expect(() => resolveModel("openai/other-model", parsed.settings.profiles, ["openai-codex"])).toThrow();
    const record = createBuildBudgetRecord(parsed, async (_argv, options) => {
      const payload = JSON.parse(options.input!);
      expect(payload.session).toBe(launch.paid!.session);
      expect(payload.binding_digest).toBe(second.digest);
      return { code: 0, stderr: "", stdout: JSON.stringify({ admitted: true }) };
    });
    expect(await record!({ kind: "budget", action: "reserve", binding_digest: second.digest })).toMatchObject({ admitted: true });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("API Build launch refuses a route without its Rust-issued financial lineage", () => {
  const dir = mkdtempSync(join(tmpdir(), "corral-build-paid-launch-"));
  const file = join(dir, "launch.json");
  const wire = { version: 1, assets: launch.assets, data_root: launch.dataRoot, corral: launch.corral,
    repo_root: launch.repoRoot, workspace_root: launch.workspaceRoot, here: false, provider: launch.provider, model: launch.model,
    route, binding, settings: { exit_policy: "none", max_parallel: 3, profiles: [] } };
  try {
    writeFileSync(file, JSON.stringify(wire));
    expect(() => readBuildLaunch(openSync(file, "r"))).toThrow("durable workspace admission");
    writeFileSync(file, JSON.stringify({ ...wire, paid: launch.paid }));
    expect(readBuildLaunch(openSync(file, "r")).paid?.session).toBe(launch.paid!.session);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
