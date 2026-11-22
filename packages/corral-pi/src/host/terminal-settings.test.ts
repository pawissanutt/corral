import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { authDir, corralPiRoot } from "../auth/paths.ts";
import { FileCredentialStore } from "../auth/store.ts";
import { terminalSettings } from "./terminal-settings.ts";
import { resolveStartupModel, type StartupModel } from "./startup-model.ts";

test("reopening settings during catalog validation cannot change the pending save's revision", async () => {
  const pending = Promise.withResolvers<StartupModel[]>();
  const selected = { selector: "openai-codex/next", provider: "openai-codex", id: "next", label: "ChatGPT subscription", savable: true } as const;
  let disk = { revision: "old", model: "openai-codex/old" };
  const settings = terminalSettings("build", "/fixture", "test-engine", { io: {
    readWorkspaceStartup: async () => disk,
    listStartupModels: () => pending.promise,
    saveWorkspaceStartup: async (_root, _corral, expected, model) => {
      if (expected.revision !== disk.revision) throw new Error("changed elsewhere");
      disk = { revision: "written", model: model ?? "" };
      return disk;
    },
  } });
  await settings.read();
  const saving = settings.setStartupModel(selected.selector);
  disk = { revision: "external", model: "openai-codex/external" };
  await settings.read();
  pending.resolve([selected]);
  await expect(saving).rejects.toThrow("changed elsewhere");
  expect(disk.revision).toBe("external");
});

test("outside-workspace Build settings save a personal choice from the catalog this Build launches with", async () => {
  const data = mkdtempSync(join(tmpdir(), "pi-startup-catalog-"));
  const previous = process.env.XDG_DATA_HOME;
  process.env.XDG_DATA_HOME = data;
  try {
    await new FileCredentialStore(authDir()).modify("openai-codex", async () => ({
      type: "oauth", access: "synthetic", refresh: "synthetic", expires: Date.now() + 60_000,
    }));
    const custom = join(data, "custom-models.json");
    const writeCatalog = (path: string, id: string) => writeFileSync(path,
      JSON.stringify({ providers: { "openai-codex": { models: [{ id }] } } }));
    writeCatalog(join(corralPiRoot(), "models.json"), "global-example");
    writeCatalog(custom, "build-example");

    const settings = terminalSettings("build", undefined, "/explicit-corral", { modelsPath: custom });
    expect((await settings.read()).workspace).toMatchObject({ editable: true, startupModel: null });
    const options = (await settings.listStartupModels()).map(model => model.selector);
    expect(options).not.toContain("openai-codex/global-example");
    expect(options).toContain("openai-codex/build-example");
    await settings.setStartupModel("openai-codex/build-example");
    expect(await resolveStartupModel({ role: "build", headless: true, modelsPath: custom })).toEqual({
      provider: "openai-codex", model: "openai-codex/build-example",
    });
  } finally {
    if (previous === undefined) delete process.env.XDG_DATA_HOME; else process.env.XDG_DATA_HOME = previous;
    rmSync(data, { recursive: true, force: true });
  }
});
