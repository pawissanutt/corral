import { afterAll, beforeAll, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BARE_DEFAULT_MODEL, BareArgsInvalid, openBare, parseBareArgs } from "../src/host/bare.ts";
import { UPSTREAM_TOOLS } from "../src/roles/types.ts";
import { type FixtureInstall, buildFixtureInstall } from "./support/fixture-install.ts";

const originalFetch = globalThis.fetch;
let assets: FixtureInstall;
let work: string;
let cwd: string;

beforeAll(() => {
  assets = buildFixtureInstall();
  work = mkdtempSync(join(tmpdir(), "corral-pi-bare-"));
  cwd = join(work, "project");
  mkdirSync(cwd, { recursive: true });
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

test("the bare entry takes argv only: a launch fd is refused and the provider default applies", () => {
  const env = { XDG_DATA_HOME: join(work, "data") };
  const options = parseBareArgs(["--cwd", cwd, "--provider", "anthropic"], assets.dir, env);
  expect(options.model).toBe(BARE_DEFAULT_MODEL.anthropic);
  expect(options.dataRoot).toBe(join(work, "data", "corral", "pi-native"));
  expect(parseBareArgs(["--cwd", cwd], assets.dir, env).provider).toBe("openai-codex");
  expect(() => parseBareArgs(["--cwd", cwd, "--launch-fd", "3"], assets.dir, env)).toThrow(BareArgsInvalid);
  expect(() => parseBareArgs(["--cwd", "relative"], assets.dir, env)).toThrow(BareArgsInvalid);
  expect(() => parseBareArgs(["--cwd", cwd, "--provider", "mistral"], assets.dir, env)).toThrow(BareArgsInvalid);
});

test("bare Pi starts unbound with only Pi's own tools, guarded, and writes no corral state", async () => {
  const dataRoot = join(work, "data", "corral", "pi-native");
  const options = parseBareArgs(["--cwd", cwd, "--model", "gpt-6.1-sol"], assets.dir, { XDG_DATA_HOME: join(work, "data") });
  const bare = await openBare(options, () => import("@earendil-works/pi-coding-agent"));
  const session = bare.runtime.session;
  const tools = session.getAllTools().map((tool) => tool.name);
  expect(tools.filter((name) => name.startsWith("corral_"))).toEqual([]);
  expect([...session.getActiveToolNames()].sort()).toEqual([...UPSTREAM_TOOLS].sort());
  expect(bare.sessionDir).toBe(join(dataRoot, "bare", "sessions"));
  expect(existsSync(join(cwd, ".corral"))).toBeFalse();
  await expect(globalThis.fetch("https://example.invalid/")).rejects.toThrow("network_refused");
  expect(bare.network.ledger().refused).toBe(1);
}, 60_000);
