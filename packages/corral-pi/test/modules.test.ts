import { afterAll, expect, test } from "bun:test";
import { existsSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import auth from "../src/auth/index.ts";
import delivery from "../src/delivery/index.ts";
import { setBundle } from "../src/host/bundle.ts";
import { ModuleInvalid, ModuleUnavailable, loadModules, missingModules, modulePaths } from "../src/host/modules.ts";
import { pmRole } from "../src/roles/pm/index.ts";
import { reviewerRole } from "../src/roles/reviewer/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import type { Role } from "../src/roles/types.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";

const installs: FixtureInstall[] = [];
const install = (options?: Parameters<typeof buildFixtureInstall>[0]) => {
  const built = buildFixtureInstall(options);
  installs.push(built);
  return built.dir;
};
afterAll(() => installs.forEach((built) => built.cleanup()));

test("the package ships the worker, reviewer, PM, delivery and auth modules", () => {
  expect(missingModules(PACKAGE_ROOT, "worker")).toEqual([]);
  expect(missingModules(PACKAGE_ROOT, "reviewer")).toEqual([]);
  expect(missingModules(PACKAGE_ROOT, "pm")).toEqual([]);
  for (const file of ["src/roles/types.ts", "src/delivery/types.ts", "src/auth/types.ts"]) {
    expect(existsSync(join(PACKAGE_ROOT, file))).toBeTrue();
  }
});

test("a fixture installation loads every role's modules from the fixed paths", async () => {
  const dir = install();
  const fixture = JSON.parse(readFileSync(join(dir, "fixture.json"), "utf8"));
  expect(fixture.fixture).toBeTrue();
  for (const role of ["worker", "reviewer", "pm"] as Role[]) {
    const loaded = await loadModules(dir, role);
    expect(loaded.role.role).toBe(role);
    expect(loaded.role.upstreamTools).toEqual(["read"]);
    expect(loaded.role.corralOps).toEqual(["hello", "event", "receipt_lookup"]);
    expect(loaded.auth.provider).toBe("openai-codex");
    expect(typeof loaded.delivery.accept).toBe("function");
  }
  expect(modulePaths("reviewer")).toEqual({
    role: "src/roles/reviewer/index.ts", delivery: "src/delivery/index.ts", auth: "src/auth/index.ts",
  });
});

test("a compiled host loads its bundled modules, building each role on the installation directory", async () => {
  const roots: string[] = [];
  const built = (factory: (root: string) => unknown) => (root: string) => {
    roots.push(root);
    return factory(root);
  };
  setBundle({
    roles: { worker: built(workerRole), reviewer: built(reviewerRole), pm: built(pmRole) },
    delivery, auth,
  });
  try {
    expect(missingModules("/no/source/tree", "worker")).toEqual([]);
    for (const role of ["worker", "reviewer", "pm"] as Role[]) {
      const loaded = await loadModules(PACKAGE_ROOT, role, "anthropic");
      expect(loaded.role.role).toBe(role);
      expect(loaded.auth.providers).toEqual(["openai-codex", "anthropic", "amazon-bedrock"]);
    }
    expect(roots).toEqual([PACKAGE_ROOT, PACKAGE_ROOT, PACKAGE_ROOT]);
  } finally {
    setBundle(undefined);
  }
  expect(missingModules("/no/source/tree", "worker").map((m) => m.code))
    .toEqual(["role_unavailable", "delivery_unavailable", "credentials_unavailable"]);
});

test("a bare installation missing PM answers ModuleUnavailable naming its owner", async () => {
  const dir = install({ overlays: false });
  unlinkSync(join(dir, "src/roles/pm/index.ts"));
  expect(existsSync(join(dir, "fixture.json"))).toBeFalse();
  const missing = missingModules(dir, "pm");
  expect(missing.map((m) => [m.code, m.module, m.owner_child])).toEqual([
    ["role_unavailable", "src/roles/pm/index.ts", "pm-role"],
  ]);
  let error: unknown;
  try {
    await loadModules(dir, "pm");
  } catch (caught) {
    error = caught;
  }
  expect(error instanceof ModuleUnavailable).toBeTrue();
  expect((error as ModuleUnavailable).owner_child).toBe("pm-role");
});

test("a missing auth module alone is credentials_unavailable, owner host-install", async () => {
  const dir = install({ omit: ["auth"] });
  let error: unknown;
  try {
    await loadModules(dir, "pm");
  } catch (caught) {
    error = caught;
  }
  expect((error as ModuleUnavailable).code).toBe("credentials_unavailable");
  expect((error as ModuleUnavailable).owner_child).toBe("host-install");
});

test("a module of the wrong shape or role is refused, and roles are a closed set", async () => {
  const dir = install();
  writeFileSync(join(dir, "src/roles/worker/index.ts"), "export default { role: \"reviewer\" };\n");
  let error: unknown;
  try {
    await loadModules(dir, "worker");
  } catch (caught) {
    error = caught;
  }
  expect(error instanceof ModuleInvalid).toBeTrue();
  expect(() => modulePaths("../../etc" as Role)).toThrow(/unknown role/);
});

test("an auth module that does not serve the bound provider is credentials_unavailable (T-479)", async () => {
  const dir = install();
  let error: unknown;
  try {
    await loadModules(dir, "worker", "anthropic");
  } catch (caught) {
    error = caught;
  }
  expect((error as ModuleUnavailable).code).toBe("credentials_unavailable");
  expect((error as ModuleUnavailable).message).toContain("for provider anthropic");
  expect((await loadModules(dir, "worker", "openai-codex")).auth.provider).toBe("openai-codex");
  const shipped = await import("../src/auth/index.ts");
  expect(shipped.default.providers).toEqual(["openai-codex", "anthropic", "amazon-bedrock"]);
});
