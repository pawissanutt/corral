// The offline model listing harness detection runs (T-503). `corral agent install` never copies
// this file.
import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { listModels } from "./models.ts";

const ENTRY = join(dirname(fileURLToPath(import.meta.url)), "models.ts");

describe("model listing", () => {
  test("lists only the requested providers' built-in models on their own model API", async () => {
    const models = await listModels(["openai-codex"]);
    expect(models.length).toBeGreaterThan(0);
    for (const model of models) {
      expect(model.provider).toBe("openai-codex");
      expect(model.selector).toBe(`openai-codex/${model.id}`);
    }
    expect(new Set(models.map((m) => m.selector)).size).toBe(models.length);
  });

  test("the entry point prints provider-qualified selectors offline and refuses unknown providers", () => {
    const run = (...args: string[]) => spawnSync(process.execPath, ["--no-env-file", ENTRY, ...args], {
      encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    });
    const listed = run("--provider", "anthropic");
    expect(listed.status).toBe(0);
    const { models } = JSON.parse(listed.stdout) as { models: { selector: string; provider: string }[] };
    expect(models.length).toBeGreaterThan(0);
    expect(models.every((m) => m.provider === "anthropic" && m.selector.startsWith("anthropic/"))).toBe(true);

    const unknown = run("--provider", "mistral");
    expect(unknown.status).toBe(2);
    expect(unknown.stderr).toContain("unknown provider mistral");
    expect(run().status).toBe(2);
  });
});
