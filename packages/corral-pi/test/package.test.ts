import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { checkRuntime, readCompat } from "../src/host/runtime.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const REPO = join(PACKAGE_ROOT, "../..");
const json = (path: string) => JSON.parse(readFileSync(path, "utf8"));
const sha256 = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");
const go = json(join(REPO, "tools/pi-probe/bun-go.json"));

test("package.json pins exactly the GO manifest's Pi release and the probe's dev pins", () => {
  const pkg = json(join(PACKAGE_ROOT, "package.json"));
  expect(go.verdict).toBe("GO");
  expect(pkg.dependencies).toEqual({ [go.pi.package]: go.pi.version });
  expect(pkg.devDependencies).toEqual(json(join(REPO, "tools/pi-probe/package.json")).devDependencies);
  expect(pkg.trustedDependencies).toEqual([]);
  expect(pkg.scripts.typecheck).toBe("tsc --noEmit -p tsconfig.json");
});

test("bun.lock is the GO lock byte for byte", () => {
  expect(sha256(join(PACKAGE_ROOT, "bun.lock"))).toBe(go.lock.sha256);
  expect(sha256(join(REPO, go.lock.file))).toBe(go.lock.sha256);
});

test("compat.json is derived from bun-go.json", () => {
  const compat = json(join(PACKAGE_ROOT, "compat.json"));
  expect(compat.verdict).toBe(go.verdict);
  expect(compat.source).toEqual({
    file: "tools/pi-probe/bun-go.json", sha256: sha256(join(REPO, "tools/pi-probe/bun-go.json")),
    task: go.task, plan: go.plan,
  });
  expect(compat.bun).toEqual({ range: go.bun.range, tested: go.bun.tested });
  for (const key of ["package", "version", "integrity", "upstreamCommit", "packages"]) {
    expect(compat.pi[key]).toEqual(go.pi[key]);
  }
  expect(compat.lock).toEqual({ file: "bun.lock", sha256: go.lock.sha256 });
});

test("the frozen install satisfies the startup pins and an unpinned Bun is refused", () => {
  const compat = readCompat(PACKAGE_ROOT);
  expect(checkRuntime(PACKAGE_ROOT, compat).pi).toBe(go.pi.version);
  expect(() => checkRuntime(PACKAGE_ROOT, compat, "1.4.1")).toThrow(/bun_version_unsupported: bun 1\.4\.1 is outside/);
  expect(() => checkRuntime(PACKAGE_ROOT, { ...compat, lock: { ...compat.lock, sha256: "0".repeat(64) } }))
    .toThrow(/lock_digest_mismatch/);
});
