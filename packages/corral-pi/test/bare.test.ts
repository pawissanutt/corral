import { afterAll, beforeAll, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { BARE_DEFAULT_MODEL, BareArgsInvalid, openBare, parseBareArgs } from "../src/host/bare.ts";
import { UPSTREAM_TOOLS } from "../src/roles/types.ts";
import { setBundle } from "../src/host/bundle.ts";
import fixtureAuth from "./fixtures/auth/index.ts";
import type { AuthContext } from "../src/auth/types.ts";
import { type FixtureInstall, PACKAGE_ROOT, buildFixtureInstall } from "./support/fixture-install.ts";

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

const ANTHROPIC_AUTH = `const credential = { type: "oauth", access: "sk-ant-oat-synthetic", refresh: "r", expires: Date.now() + 3_600_000 };
export default {
  provider: "openai-codex", providers: ["openai-codex", "anthropic"],
  credentials() {
    return {
      async read(provider) { return provider === "anthropic" ? credential : undefined; },
      async list() { return [{ providerId: "anthropic", type: "oauth" }]; },
      async modify() { throw new Error("read-only"); },
      async delete() { throw new Error("read-only"); },
    };
  },
};
`;

test("bare Anthropic runs through the bound host's Claude Agent SDK lane with only the refresh endpoint open", async () => {
  const install = buildFixtureInstall({ overlays: false });
  try {
    writeFileSync(join(install.dir, "src/auth/index.ts"), ANTHROPIC_AUTH);
    const options = parseBareArgs(["--cwd", cwd, "--provider", "anthropic"], install.dir, { XDG_DATA_HOME: join(work, "data-anthropic") });
    const bare = await openBare(options, () => import("@earendil-works/pi-coding-agent"));
    expect(bare.runtime.session.getAllTools().some((tool) => tool.name.startsWith("corral_"))).toBeFalse();
    await expect(globalThis.fetch("https://api.anthropic.com/v1/messages", { method: "POST" })).rejects.toThrow("network_refused");
  } finally {
    install.cleanup();
  }
}, 60_000);

test("a compiled installation's bare mode uses the bundled auth: its directory has no src/ tree", async () => {
  const compiled = mkdtempSync(join(work, "compiled-"));
  copyFileSync(join(PACKAGE_ROOT, "compat.json"), join(compiled, "compat.json"));
  setBundle({ auth: fixtureAuth });
  try {
    expect(existsSync(join(compiled, "src"))).toBeFalse();
    const options = parseBareArgs(["--cwd", cwd], compiled, { XDG_DATA_HOME: join(work, "data-compiled") });
    const bare = await openBare(options, () => import("@earendil-works/pi-coding-agent"));
    const tools = bare.runtime.session.getAllTools().map((tool) => tool.name);
    expect(tools.filter((name) => name.startsWith("corral_"))).toEqual([]);
    expect([...bare.runtime.session.getActiveToolNames()].sort()).toEqual([...UPSTREAM_TOOLS].sort());
  } finally {
    setBundle(undefined);
  }
}, 60_000);

test("bare model selection switches from OpenAI to Claude and back in the same session", async () => {
  // Given both provider logins, without contacting either provider.
  setBundle({ auth: {
    ...fixtureAuth, providers: ["openai-codex", "anthropic"],
    credentials(context: AuthContext) {
      if (context.provider === "openai-codex") return fixtureAuth.credentials(context);
      return {
        async read(provider: string) {
          return provider === "anthropic" ? {
            type: "oauth" as const, access: "sk-ant-oat-synthetic", refresh: "r", expires: 4_000_000_000_000,
          } : undefined;
        },
        async list() { return [{ providerId: "anthropic", type: "oauth" as const }]; },
        async modify() { throw new Error("read-only"); },
        async delete() { throw new Error("read-only"); },
      };
    },
  } });
  try {
    const options = parseBareArgs(["--cwd", cwd], assets.dir, { XDG_DATA_HOME: join(work, "data-switch") });
    mkdirSync(options.dataRoot, { recursive: true });
    writeFileSync(join(options.dataRoot, "models.json"), JSON.stringify({
      providers: { anthropic: { models: [{ id: "configured-future-claude" }] } },
    }));
    const bare = await openBare(options, () => import("@earendil-works/pi-coding-agent"));
    const session = bare.runtime.session;
    // When the real runtime refreshes the picker and selects Claude.
    await session.modelRuntime.refresh({ allowNetwork: false });
    const available = await session.modelRuntime.getAvailable();
    const claude = available.find((model) => model.provider === "anthropic" && model.id === "configured-future-claude");
    const openai = available.find((model) => model.provider === "openai-codex");
    if (!claude || !openai) throw new Error("both provider catalogs must be available");
    await session.setModel(claude);
    // Then the selected provider changes in place, without replacing the session.
    expect(session.model?.provider).toBe("anthropic");
    await session.setModel(openai);
    expect(session.model?.provider).toBe("openai-codex");
    expect(session.modelRuntime.getProvider("anthropic")?.auth.apiKey).toBeUndefined();
    expect(bare.network.ledger().allowed).toBe(0);
  } finally {
    setBundle(undefined);
  }
}, 60_000);

test("the compiled entry routes `bare` to the bare entry, never to the bound host", () => {
  const child = spawnSync(process.execPath, ["--no-env-file", "src/host/compiled.ts", "bare", "--cwd", "relative"], {
    cwd: PACKAGE_ROOT, stdio: ["ignore", "pipe", "pipe"], encoding: "utf8",
    env: { ...process.env, XDG_DATA_HOME: join(work, "data-dispatch") },
  });
  const stderr = child.stderr;
  expect(child.status).toBe(2);
  expect(stderr).toContain("usage_error: --cwd must be an absolute directory");
  expect(stderr).not.toContain("launch_message_invalid");
}, 60_000);

function compiledEntryTrace(args: readonly string[]): { status: number | null; stderr: string; loaded: string[] } {
  const trace = join(mkdtempSync(join(work, "trace-")), "modules.log");
  writeFileSync(trace, "");
  const child = spawnSync(process.execPath,
    ["--no-env-file", "--preload", "./test/support/module-trace.ts", "src/host/compiled.ts", ...args], {
      cwd: PACKAGE_ROOT, stdio: ["ignore", "pipe", "pipe"], encoding: "utf8",
      env: { ...process.env, XDG_DATA_HOME: join(work, "data-trace"), CORRAL_MODULE_TRACE: trace },
    });
  const loaded = readFileSync(trace, "utf8").split("\n").filter(Boolean)
    .map((path) => path.slice(path.indexOf("/src/") + 1));
  return { status: child.status, stderr: child.stderr, loaded };
}

test("compiled bare startup initializes no role or delivery module; the bound host still loads all four", () => {
  const bare = compiledEntryTrace(["bare", "--cwd", "relative"]);
  expect(bare.stderr).toContain("usage_error: --cwd must be an absolute directory");
  expect(bare.loaded).toEqual([]);
  const bound = compiledEntryTrace(["--launch-fd", "99"]);
  expect([...new Set(bound.loaded)].sort()).toEqual([
    "src/delivery/index.ts", "src/roles/pm/index.ts", "src/roles/reviewer/index.ts", "src/roles/worker/index.ts",
  ]);
}, 60_000);
