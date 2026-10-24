import { afterAll, expect, test } from "bun:test";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { ModuleInvalid, ModuleUnavailable, loadModules, missingModules, modulePaths } from "../src/host/modules.ts";
import type { Role } from "../src/roles/types.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";

const installs: FixtureInstall[] = [];
const install = (options?: Parameters<typeof buildFixtureInstall>[0]) => {
  const built = buildFixtureInstall(options);
  installs.push(built);
  return built.dir;
};
afterAll(() => installs.forEach((built) => built.cleanup()));

test("the package itself ships no role, delivery or auth module before their owning children land", () => {
  expect(missingModules(PACKAGE_ROOT, "worker").map((m) => m.message)).toEqual([
    "role_unavailable worker: no src/roles/worker/index.ts (owner worker-role)",
    "credentials_unavailable: no src/auth/index.ts (owner host-install)",
  ]);
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

test("an installation without fixture overlays answers ModuleUnavailable naming each owner", async () => {
  const dir = install({ overlays: false });
  expect(existsSync(join(dir, "fixture.json"))).toBeFalse();
  const missing = missingModules(dir, "worker");
  expect(missing.map((m) => [m.code, m.module, m.owner_child])).toEqual([
    ["role_unavailable", "src/roles/worker/index.ts", "worker-role"],
    ["credentials_unavailable", "src/auth/index.ts", "host-install"],
  ]);
  let error: unknown;
  try {
    await loadModules(dir, "worker");
  } catch (caught) {
    error = caught;
  }
  expect(error instanceof ModuleUnavailable).toBeTrue();
  expect((error as ModuleUnavailable).owner_child).toBe("worker-role");
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
