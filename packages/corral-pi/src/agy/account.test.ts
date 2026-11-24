import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { modelSettings } from "../host/config.ts";

const cli = fileURLToPath(new URL("../cli.ts", import.meta.url));
function account(script: string | undefined, ...args: string[]) {
  const dir = mkdtempSync(join(tmpdir(), "corral-agy-test-"));
  try {
    if (script) writeFileSync(join(dir, "agy"), `#!/bin/sh\n${script}`, { mode: 0o700 });
    return spawnSync(process.execPath, ["--no-env-file", cli, ...args, "--provider", "antigravity-cli"], {
      env: { PATH: dir, HOME: dir, TMPDIR: dir, GEMINI_API_KEY: "synthetic-key", GOOGLE_GEMINI_BASE_URL: "synthetic-endpoint" },
      cwd: dir, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 5_000,
    });
  } finally { rmSync(dir, { recursive: true, force: true }); }
}

test("discovery exports exact account-visible Gemini selectors without importing ambient credentials or CLI labels", () => {
  const listed = account(`
    [ "$#" = 1 ] && [ "$1" = models ] || exit 3
    case "$PWD" in "$HOME"|"$HOME"/*) exit 3;; esac
    [ -z "\${GEMINI_API_KEY+present}" ] && [ -z "\${GOOGLE_GEMINI_BASE_URL+present}" ] || exit 4
    printf 'gemini-4-neon-high\tSynthetic label\ngemini-4-neon-low\tLow\n'
    printf 'claude-sonnet-5-5-high\tClaude\ngemini-4-neon-high\tDuplicate\ngemini-pro-orchestrator\tGemini\n'
    printf 'synthetic-error-detail\n' >&2
  `, "models");
  expect(listed.status).toBe(0);
  expect(listed.stderr).toBe("");
  expect(JSON.parse(listed.stdout).models).toEqual([
    { provider: "antigravity-cli", id: "gemini-4-neon-high", selector: "antigravity-cli/gemini-4-neon-high", effort: "high" },
    { provider: "antigravity-cli", id: "gemini-4-neon-low", selector: "antigravity-cli/gemini-4-neon-low", effort: "low" },
    { provider: "antigravity-cli", id: "gemini-pro-orchestrator", selector: "antigravity-cli/gemini-pro-orchestrator" },
  ]);
});

test("missing CLI, signed out, empty Gemini catalog and discovery failure remain distinct", () => {
  const cases = [
    [undefined, "agy_missing"],
    [`echo 'Error: Please sign in to view available models. Launch the CLI without arguments to sign in.' >&2; exit 1`, "agy_signed_out"],
    [`printf 'claude-sonnet-5-5-high\tClaude\n'`, "agy_no_models"],
    [`echo 'synthetic-error-secret' >&2; exit 1`, "agy_catalog_unavailable"],
  ] as const;
  for (const [script, code] of cases) {
    const result = account(script, "models");
    expect(result.status).toBe(1);
    expect(result.stdout).toBe("");
    expect(result.stderr.startsWith(`corral agent models failed: ${code}:`)).toBe(true);
    expect(result.stderr).not.toContain("synthetic-error-secret");
  }
});

test("a positional Antigravity provider after -- does not opt in to discovery", () => {
  const result = account(`printf 'gemini-4-neon-high\tGemini\n'`, "models", "--provider", "openai-codex", "--");
  expect(result.status).toBe(2);
  expect(result.stdout).toBe("");
});

test("login will not launch agy in a non-interactive capture", () => {
  const result = account("exit 23", "login");
  expect(result.status).toBe(1);
  expect(result.stdout).toBe("");
  expect(result.stderr.startsWith("corral-pi login: agy_login_requires_terminal:")).toBe(true);
});

test("discovery-only IDs cannot change the default or become a launch or fallback route", () => {
  const model = "antigravity-cli/gemini-4-neon-high";
  expect(() => modelSettings({ model }, "startup")).toThrow(/discovery only/);
  expect(() => modelSettings({ provider: "antigravity-cli", model: "gemini-4-neon-high" })).toThrow(/discovery only/);
  expect(() => modelSettings({ fallback: [model] })).toThrow(/discovery only/);
  expect(modelSettings({}).provider).toBe("openai-codex");
});
