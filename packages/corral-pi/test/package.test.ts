import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { bunApi, checkCompiledRuntime, checkRuntime, readCompat } from "../src/host/runtime.ts";
import { PACKAGE_ROOT } from "./support/fixture-install.ts";

const REPO = join(PACKAGE_ROOT, "../..");
const json = (path: string) => JSON.parse(readFileSync(path, "utf8"));
const sha256 = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");
const go = json(join(REPO, "tools/pi-probe/bun-go.json"));
/** The Bedrock host's direct dependencies (T-504), each at the version the GO lock already resolves. */
const AWS_DEPENDENCIES = ["@aws-sdk/nested-clients", "@smithy/core", "@smithy/fetch-http-handler", "@smithy/node-http-handler"];

/** Local, inert HTML conversion dependencies (T-861); the GO probe's original pins stay immutable. */
const WEB_DEPENDENCIES = { "@mozilla/readability": "0.6.0", linkedom: "0.18.15", turndown: "7.2.5" };
const WEB_DEV_DEPENDENCIES = { "@types/turndown": "5.0.6" };
const WEB_LOCK_ADDITIONS = [
  "@mixmark-io/domino", "@mozilla/readability", "@types/turndown", "boolbase", "css-select", "css-what", "cssomnom",
  "dom-serializer", "domelementtype", "domhandler", "domutils", "entities", "html-escaper", "htmlparser2", "linkedom",
  "nth-check", "turndown", "uhyphen", "dom-serializer/domelementtype", "dom-serializer/entities", "domhandler/domelementtype",
  "domutils/domelementtype", "htmlparser2/domhandler", "htmlparser2/domutils", "htmlparser2/domutils/dom-serializer",
  "htmlparser2/domutils/dom-serializer/entities",
];

test("package.json pins the GO runtime, Claude lane, local HTML converters and development tools", () => {
  const pkg = json(join(PACKAGE_ROOT, "package.json"));
  const compat = json(join(PACKAGE_ROOT, "compat.json"));
  expect(go.verdict).toBe("GO");
  const goLock = bunApi().JSONC.parse(readFileSync(join(REPO, go.lock.file), "utf8")) as { packages: Record<string, string[]> };
  const locked = (name: string) => goLock.packages[name]![0]!.slice(name.length + 1);
  expect(pkg.dependencies).toEqual({
    [go.pi.package]: go.pi.version, [compat.claude.package]: compat.claude.version, zod: compat.claude.zod,
    ...Object.fromEntries(AWS_DEPENDENCIES.map((name) => [name, locked(name)])), ...WEB_DEPENDENCIES,
  });
  expect(pkg.devDependencies).toEqual({
    ...json(join(REPO, "tools/pi-probe/package.json")).devDependencies, ...WEB_DEV_DEPENDENCIES,
  });
  expect(pkg.trustedDependencies).toEqual([]);
  expect(pkg.scripts.typecheck).toBe("tsc --noEmit -p tsconfig.json");
});

test("bun.lock preserves every GO entry and only adds the Claude SDK and local HTML dependency closure", () => {
  expect(sha256(join(REPO, go.lock.file))).toBe(go.lock.sha256);
  const parse = (path: string) => bunApi().JSONC.parse(readFileSync(path, "utf8")) as { packages: Record<string, unknown[]> };
  const goLock = parse(join(REPO, go.lock.file));
  const lock = parse(join(PACKAGE_ROOT, "bun.lock"));
  for (const [name, entry] of Object.entries(goLock.packages)) expect([name, lock.packages[name]]).toEqual([name, entry]);
  const added = Object.keys(lock.packages).filter((name) => !(name in goLock.packages));
  expect(added.filter((name) => !name.startsWith("@anthropic-ai/claude-agent-sdk")).sort())
    .toEqual([...WEB_LOCK_ADDITIONS].sort());
  for (const [name, version] of Object.entries({ ...WEB_DEPENDENCIES, ...WEB_DEV_DEPENDENCIES })) {
    expect(lock.packages[name]?.[0]).toBe(`${name}@${version}`);
  }
  const compat = json(join(PACKAGE_ROOT, "compat.json"));
  const sdk = lock.packages[compat.claude.package]!;
  expect([sdk[0], sdk[3]]).toEqual([`${compat.claude.package}@${compat.claude.version}`, compat.claude.integrity]);
  expect(lock.packages.zod?.[0]).toBe(`zod@${compat.claude.zod}`);
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
  expect(compat.lock).toEqual({ file: "bun.lock", sha256: sha256(join(PACKAGE_ROOT, "bun.lock")) });
});

test("a compiled host checks only its embedded Bun against the tested range", () => {
  const compat = readCompat(PACKAGE_ROOT);
  expect(checkCompiledRuntime(compat, compat.bun.range.replace(/^[^0-9]*/, "")))
    .toEqual({ bun: compat.bun.range.replace(/^[^0-9]*/, ""), pi: compat.pi.version, lockSha256: compat.lock.sha256 });
  expect(() => checkCompiledRuntime(compat, "1.4.1")).toThrow(/bun_version_unsupported: the embedded bun 1\.4\.1/);
});

test("the frozen install satisfies the startup pins and an unpinned Bun is refused", () => {
  const compat = readCompat(PACKAGE_ROOT);
  expect(checkRuntime(PACKAGE_ROOT, compat).pi).toBe(go.pi.version);
  expect(() => checkRuntime(PACKAGE_ROOT, compat, "1.4.1")).toThrow(/bun_version_unsupported: bun 1\.4\.1 is outside/);
  expect(() => checkRuntime(PACKAGE_ROOT, { ...compat, lock: { ...compat.lock, sha256: "0".repeat(64) } }))
    .toThrow(/lock_digest_mismatch/);
});
