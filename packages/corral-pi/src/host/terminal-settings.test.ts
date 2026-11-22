import { expect, test } from "bun:test";
import { terminalSettings } from "./terminal-settings.ts";
import type { StartupModel } from "./startup-model.ts";

test("reopening settings during catalog validation cannot change the pending save's revision", async () => {
  const pending = Promise.withResolvers<StartupModel[]>();
  const selected = { selector: "openai-codex/next", provider: "openai-codex", id: "next", label: "ChatGPT subscription", savable: true } as const;
  let disk = { revision: "old", model: "openai-codex/old" };
  const settings = terminalSettings("build", "/fixture", "test-engine", {
    readWorkspaceStartup: async () => disk,
    listStartupModels: () => pending.promise,
    saveWorkspaceStartup: async (_root, _corral, expected, model) => {
      if (expected.revision !== disk.revision) throw new Error("changed elsewhere");
      disk = { revision: "written", model: model ?? "" };
      return disk;
    },
  });
  await settings.read();
  const saving = settings.setStartupModel(selected.selector);
  disk = { revision: "external", model: "openai-codex/external" };
  await settings.read();
  pending.resolve([selected]);
  await expect(saving).rejects.toThrow("changed elsewhere");
  expect(disk.revision).toBe("external");
});
