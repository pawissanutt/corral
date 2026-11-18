import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { JSONC } from "bun";
import { checkRuntime } from "../src/host/runtime.ts";
import pkg from "../package.json" with { type: "json" };

test("a source host accepts the pinned Bun and refuses versions outside the package range", () => {
  expect(() => checkRuntime()).not.toThrow();
  expect(() => checkRuntime("0.0.1")).toThrow(`outside ${pkg.engines.bun}`);
});

test("the package pins its runtime dependencies and its Pi patch targets agree with the frozen lock", () => {
  const lock = JSONC.parse(readFileSync(new URL("../bun.lock", import.meta.url), "utf8")) as {
    workspaces: Record<string, { dependencies: unknown; devDependencies: unknown }>;
    packages: Record<string, [string, ...unknown[]]>;
    patchedDependencies: unknown;
  };
  expect(lock.workspaces[""]?.dependencies).toEqual(pkg.dependencies);
  expect(lock.workspaces[""]?.devDependencies).toEqual(pkg.devDependencies);
  expect(lock.patchedDependencies).toEqual(pkg.patchedDependencies);
  for (const target of Object.keys(pkg.patchedDependencies)) {
    expect(lock.packages[target.slice(0, target.lastIndexOf("@"))]?.[0]).toBe(target);
  }
});
