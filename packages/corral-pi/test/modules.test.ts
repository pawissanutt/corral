import { afterAll, expect, test } from "bun:test";
import { existsSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import auth from "../src/auth/index.ts";
import { setBundle } from "../src/host/bundle.ts";
import { ModuleInvalid, ModuleUnavailable, loadModules, missingModules, modulePaths } from "../src/host/modules.ts";
import { assistantRole } from "../src/roles/assistant/index.ts";
import { buildRole } from "../src/roles/build/index.ts";
import { reviewerRole } from "../src/roles/reviewer/index.ts";
import { workerRole } from "../src/roles/worker/index.ts";
import { ROLES, type Role } from "../src/roles/types.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";

const installs: FixtureInstall[] = [];
const install = (options?: Parameters<typeof buildFixtureInstall>[0]) => {
  const built = buildFixtureInstall(options);
  installs.push(built);
  return built.dir;
};
afterAll(() => installs.forEach((built) => built.cleanup()));

test("the package ships every role and the auth module", () => {
  expect(ROLES).toEqual(["worker", "reviewer", "assistant", "build"]);
  for (const role of ROLES) expect(missingModules(PACKAGE_ROOT, role)).toEqual([]);
  for (const file of ["src/roles/types.ts", "src/auth/types.ts"]) {
    expect(existsSync(join(PACKAGE_ROOT, file))).toBeTrue();
  }
});

test("a fixture installation loads every role's modules from the fixed paths", async () => {
  const dir = install();
  const fixture = JSON.parse(readFileSync(join(dir, "fixture.json"), "utf8"));
  expect(fixture.fixture).toBeTrue();
  for (const role of ROLES) {
    const loaded = await loadModules(dir, role);
    expect(loaded.role.role).toBe(role);
    expect(loaded.role.upstreamTools).toEqual(["read"]);
    expect(loaded.auth.provider).toBe("openai-codex");
  }
  expect(modulePaths("reviewer")).toEqual({
    role: "src/roles/reviewer/index.ts", auth: "src/auth/index.ts",
  });
});

test("a compiled host loads its bundled modules, building each role on the installation directory", async () => {
  const roots: string[] = [];
  const built = (factory: (root: string) => unknown) => (root: string) => {
    roots.push(root);
    return factory(root);
  };
  setBundle({
    roles: {
      worker: built(workerRole), reviewer: built(reviewerRole),
      assistant: built(assistantRole), build: built(buildRole),
    },
    auth,
  });
  try {
    expect(missingModules("/no/source/tree", "worker")).toEqual([]);
    for (const role of ROLES) {
      const loaded = await loadModules(PACKAGE_ROOT, role, "anthropic");
      expect(loaded.role.role).toBe(role);
      expect(loaded.auth.providers).toEqual(["openai-codex", "anthropic", "openai", "amazon-bedrock", "llama.cpp"]);
    }
    expect(roots).toEqual(ROLES.map(() => PACKAGE_ROOT));
    expect(auth.connections).toEqual([
      "anthropic-subscription", "anthropic-api", "openai-subscription", "openai-api", "amazon-bedrock", "llama.cpp",
    ]);
    // A bundled API provider still refuses the incompatible subscription connection.
    expect(() => auth.credentials({ provider: "openai", connection: "openai-subscription", apiEndpoint: "", refreshEndpoint: "" }))
      .toThrow("connection_invalid");
  } finally {
    setBundle(undefined);
  }
  expect(missingModules("/no/source/tree", "worker").map((m) => m.code))
    .toEqual(["role_unavailable", "credentials_unavailable"]);
});

test("an installation missing the assistant answers ModuleUnavailable naming its owner", async () => {
  const dir = install({ overlays: false });
  unlinkSync(join(dir, "src/roles/assistant/index.ts"));
  expect(existsSync(join(dir, "fixture.json"))).toBeFalse();
  const missing = missingModules(dir, "assistant");
  expect(missing.map((m) => [m.code, m.module, m.owner_child])).toEqual([
    ["role_unavailable", "src/roles/assistant/index.ts", "assistant-role"],
  ]);
  let error: unknown;
  try {
    await loadModules(dir, "assistant");
  } catch (caught) {
    error = caught;
  }
  expect(error instanceof ModuleUnavailable).toBeTrue();
  expect((error as ModuleUnavailable).owner_child).toBe("assistant-role");
});

test("a missing auth module alone is credentials_unavailable, owner host-install", async () => {
  const dir = install({ omit: ["auth"] });
  let error: unknown;
  try {
    await loadModules(dir, "assistant");
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
  expect(shipped.default.providers).toEqual(["openai-codex", "anthropic", "openai", "amazon-bedrock", "llama.cpp"]);
});
