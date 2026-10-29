import { afterEach, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { listModels } from "../auth/models.ts";
import { createModelCatalog, ModelCatalogInvalid, NO_CATALOG_CREDENTIALS } from "./catalog.ts";

const directories: string[] = [];
afterEach(() => {
  for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
});

function config(models: readonly { readonly id: string; readonly api?: string }[]): string {
  const directory = mkdtempSync(join(tmpdir(), "corral-catalog-"));
  directories.push(directory);
  const path = join(directory, "models.json");
  writeConfig(path, models);
  return path;
}

function writeConfig(path: string, models: readonly { readonly id: string; readonly api?: string }[]): void {
  // Replace atomically: a reader of an existing models.json must never see a truncated file.
  const staged = `${path}.${process.pid}.tmp`;
  writeFileSync(staged, JSON.stringify({
    providers: {
      "openai-codex": {
        apiKey: "paid-config-key",
        headers: { Authorization: "Bearer config-key" },
        models,
      },
      anthropic: { apiKey: "unapproved-key", models: [{ id: "unapproved-configured-model" }] },
      "custom-paid": {
        api: "openai-completions", baseUrl: "https://not-allowed.invalid/v1", apiKey: "custom-key",
        models: [{ id: "custom-unapproved-model" }],
      },
    },
  }));
  renameSync(staged, path);
}

describe("shared model catalog", () => {
  test("defaults to corral-owned metadata instead of ambient Pi configuration", async () => {
    // Given
    const ambientPath = config([{ id: "ambient-pi-model" }]);
    const data = mkdtempSync(join(tmpdir(), "corral-catalog-data-"));
    directories.push(data);
    const owned = join(data, "corral", "pi-native");
    mkdirSync(owned, { recursive: true });
    writeConfig(join(owned, "models.json"), [{ id: "corral-owned-model" }]);
    const previousXdg = process.env.XDG_DATA_HOME;
    const previousPi = process.env.PI_CODING_AGENT_DIR;
    process.env.XDG_DATA_HOME = data;
    process.env.PI_CODING_AGENT_DIR = join(ambientPath, "..");
    try {
      // When
      const models = await listModels(["openai-codex"]);
      // Then
      expect(models.some((model) => model.id === "corral-owned-model")).toBe(true);
      expect(models.some((model) => model.id === "ambient-pi-model")).toBe(false);
      expect(existsSync(join(owned, "models-store.json"))).toBe(false);
      expect(existsSync(join(ambientPath, "..", "models-store.json"))).toBe(false);
    } finally {
      if (previousXdg === undefined) delete process.env.XDG_DATA_HOME;
      else process.env.XDG_DATA_HOME = previousXdg;
      if (previousPi === undefined) delete process.env.PI_CODING_AGENT_DIR;
      else process.env.PI_CODING_AGENT_DIR = previousPi;
    }
  });

  test("rejects malformed metadata on startup with its file diagnostic", async () => {
    // Given
    const modelsPath = config([]);
    writeFileSync(modelsPath, "{broken");
    // When / Then
    await expect(createModelCatalog(NO_CATALOG_CREDENTIALS, {
      providers: ["openai-codex"], modelsPath,
    })).rejects.toThrow(ModelCatalogInvalid);
    await expect(listModels(["openai-codex"], { modelsPath })).rejects.toThrow(modelsPath);
  });

  test("retains the valid catalog when refresh encounters invalid configuration", async () => {
    // Given
    const modelsPath = config([{ id: "last-valid-model" }]);
    const runtime = await createModelCatalog(NO_CATALOG_CREDENTIALS, { providers: ["openai-codex"], modelsPath });
    writeFileSync(modelsPath, JSON.stringify({ providers: { "openai-codex": { models: [{ id: 42 }] } } }));
    // When
    await expect(runtime.refresh()).rejects.toThrow(ModelCatalogInvalid);
    // Then
    expect(runtime.getModel("openai-codex", "last-valid-model")?.id).toBe("last-valid-model");
  });

  test("lists a new model from SDK configuration without adding model IDs to code", async () => {
    // Given
    const modelsPath = config([{ id: "configured-future-model" }]);
    // When
    const models = await listModels(["openai-codex"], { modelsPath });
    // Then
    expect(models.find((model) => model.id === "configured-future-model")).toEqual({
      selector: "openai-codex/configured-future-model", provider: "openai-codex", id: "configured-future-model",
    });
    expect(models.every((model) => model.provider === "openai-codex")).toBe(true);
  });

  test("refresh reloads configuration while preserving native stream restrictions offline", async () => {
    // Given
    const modelsPath = config([{ id: "before-refresh" }]);
    const runtime = await createModelCatalog(NO_CATALOG_CREDENTIALS, { providers: ["openai-codex"], modelsPath });
    const provider = runtime.getProvider("openai-codex");
    if (!provider) throw new Error("missing test provider");
    const stream = () => { throw new Error("guarded-stream-sentinel"); };
    runtime.registerNativeProvider({ ...provider, stream, streamSimple: stream });
    writeConfig(modelsPath, [{ id: "after-refresh" }, { id: "wrong-api", api: "openai-completions" }]);
    const fetch = globalThis.fetch;
    let requests = 0;
    globalThis.fetch = async () => {
      requests += 1;
      throw new Error("network must remain offline");
    };
    try {
      // When
      await runtime.refresh({ allowNetwork: true, force: true });
      // Then
      expect(runtime.getModel("openai-codex", "after-refresh")?.id).toBe("after-refresh");
      expect(runtime.getModel("openai-codex", "before-refresh")).toBeUndefined();
      expect(runtime.getModel("openai-codex", "wrong-api")).toBeUndefined();
      expect(runtime.getProvider("openai-codex")?.stream).toBe(stream);
      expect(runtime.getProvider("openai-codex")?.auth.apiKey).toBeUndefined();
      expect(runtime.getProvider("openai-codex")?.headers?.["Authorization"]).toBeUndefined();
      expect(requests).toBe(0);
    } finally {
      globalThis.fetch = fetch;
    }
  });

  test("unapproved and configured paid providers stay unavailable after refresh", async () => {
    // Given
    const modelsPath = config([{ id: "approved-model" }]);
    const previous = process.env.ANTHROPIC_API_KEY;
    process.env.ANTHROPIC_API_KEY = "ambient-paid-key";
    try {
      const runtime = await createModelCatalog(NO_CATALOG_CREDENTIALS, { providers: ["openai-codex"], modelsPath });
      // When
      await runtime.refresh();
      // Then
      expect(runtime.getModels().every((model) => model.provider === "openai-codex")).toBe(true);
      expect(runtime.getModels("anthropic")).toEqual([]);
      expect(runtime.getModels("custom-paid")).toEqual([]);
      expect(await runtime.checkAuth("anthropic")).toBeUndefined();
      expect(await runtime.getAvailable()).toEqual([]);
      expect(await runtime.checkAuth("openai-codex")).toBeUndefined();
    } finally {
      if (previous === undefined) delete process.env.ANTHROPIC_API_KEY;
      else process.env.ANTHROPIC_API_KEY = previous;
    }
  });
});

describe("supplemental models", () => {
  test("resolves gpt-6.1-terra without models.json and still refuses unknown models", async () => {
    // Given
    const runtime = await createModelCatalog(NO_CATALOG_CREDENTIALS, { providers: ["openai-codex"], modelsPath: null });
    // When
    const terra = runtime.getModel("openai-codex", "gpt-6.1-terra");
    // Then
    expect(terra?.contextWindow).toBe(500000);
    expect(terra?.reasoning).toBe(true);
    expect(runtime.getModel("openai-codex", "gpt-9-missing")).toBeUndefined();
    expect((await listModels(["openai-codex"], { modelsPath: null })).map((m) => m.selector))
      .toContain("openai-codex/gpt-6.1-terra");
  });
});
