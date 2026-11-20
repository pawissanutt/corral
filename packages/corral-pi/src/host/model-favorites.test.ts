import { expect, test } from "bun:test";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { WorkspaceModelFavorites } from "./model-favorites.ts";
import { RemoteRegistry } from "./remote/registry.ts";
import { registerModels, type SessionModels } from "./remote/models.ts";

const claude = { provider: "anthropic", id: "claude-sonnet-4" };

async function until(check: () => boolean): Promise<boolean> {
  for (let tries = 0; tries < 100 && !check(); tries++) await new Promise(resolve => setTimeout(resolve, 10));
  return check();
}

test("favorites persist in the workspace, per provider and id, and the app's command marks without switching", async () => {
  const root = mkdtempSync(join(tmpdir(), "favorites-"));
  try {
    new WorkspaceModelFavorites(root).set(claude, true);
    const restarted = new WorkspaceModelFavorites(root);
    expect(restarted.has(claude)).toBeTrue();
    expect(restarted.has({ provider: "anthropic-api", id: "claude-sonnet-4" })).toBeFalse();

    const registry = new RemoteRegistry();
    registerModels(registry, () => ({ models: [], refreshing: false }) as SessionModels, restarted);
    await registry.get("model_favorite")!({ model: "openrouter/anthropic/claude-sonnet-4", favorite: true });
    expect(new WorkspaceModelFavorites(root).has({ provider: "openrouter", id: "anthropic/claude-sonnet-4" })).toBeTrue();
    expect(() => registry.get("model_favorite")!({ model: "no-provider", favorite: true })).toThrow();

    // A damaged favorite reads as none instead of failing the catalog.
    const damaged = new WorkspaceModelFavorites(root);
    for (const name of ["x.json", ...readdirSync(damaged.dir)]) writeFileSync(join(damaged.dir, name), "{not json");
    expect(new WorkspaceModelFavorites(root).has(claude)).toBeFalse();
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test("another host's change notifies a watching host, and concurrent hosts never lose one another's favorites", async () => {
  const root = mkdtempSync(join(tmpdir(), "favorites-"));
  try {
    let notified = 0;
    const watching = new WorkspaceModelFavorites(root, () => notified++);
    // Two hosts with the same (empty) view each save a different favorite.
    const a = new WorkspaceModelFavorites(root), b = new WorkspaceModelFavorites(root);
    expect(a.has(claude) || b.has(claude)).toBeFalse();
    a.set(claude, true);
    b.set({ provider: "openai", id: "gpt-5" }, true);
    expect(await until(() => notified > 0)).toBeTrue();
    expect(watching.has(claude)).toBeTrue();
    expect(watching.has({ provider: "openai", id: "gpt-5" })).toBeTrue();
    // A host that crashed mid-save leaves only a temporary file, which is no favorite and blocks nobody.
    writeFileSync(join(watching.dir, ".999.dead.tmp"), "{");
    b.set(claude, false);
    expect(await until(() => !watching.has(claude))).toBeTrue();
    expect(watching.has({ provider: "openai", id: "gpt-5" })).toBeTrue();
    watching.close();
  } finally { rmSync(root, { recursive: true, force: true }); }
});
