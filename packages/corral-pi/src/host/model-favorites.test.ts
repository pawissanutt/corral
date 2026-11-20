import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { WorkspaceModelFavorites } from "./model-favorites.ts";
import { RemoteRegistry } from "./remote/registry.ts";
import { registerModels, type SessionModels } from "./remote/models.ts";

test("favorites persist in the workspace for every host, per provider and id, and the app's command marks without switching", async () => {
  const root = mkdtempSync(join(tmpdir(), "favorites-"));
  try {
    let changes = 0;
    const first = new WorkspaceModelFavorites(root, () => changes++);
    first.set({ provider: "anthropic", id: "claude-sonnet-4" }, true);
    const restarted = new WorkspaceModelFavorites(root);
    expect(restarted.has({ provider: "anthropic", id: "claude-sonnet-4" })).toBeTrue();
    expect(restarted.has({ provider: "anthropic-api", id: "claude-sonnet-4" })).toBeFalse();

    const registry = new RemoteRegistry();
    registerModels(registry, () => ({ models: [], refreshing: false }) as SessionModels, restarted);
    await registry.get("model_favorite")!({ model: "openrouter/anthropic/claude-sonnet-4", favorite: true });
    expect(first.has({ provider: "openrouter", id: "anthropic/claude-sonnet-4" })).toBeTrue();
    expect(() => registry.get("model_favorite")!({ model: "no-provider", favorite: true })).toThrow();
    first.set({ provider: "anthropic", id: "claude-sonnet-4" }, false);
    expect(restarted.has({ provider: "anthropic", id: "claude-sonnet-4" })).toBeFalse();
    expect(changes).toBe(2);

    // A damaged file reads as no favorites instead of failing the catalog.
    writeFileSync(first.path, "{not json");
    expect(new WorkspaceModelFavorites(root).has({ provider: "openrouter", id: "anthropic/claude-sonnet-4" })).toBeFalse();
  } finally { rmSync(root, { recursive: true, force: true }); }
});
