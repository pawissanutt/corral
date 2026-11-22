import { basename } from "node:path";
import type { HostIdentity } from "../roles/types.ts";
import type { SettingsControl, WorkspaceSettings } from "../ui/settings-view.ts";
import { readPersonalPreferences, savePersonalPreference } from "./preferences.ts";
import { listStartupModels } from "./startup-model.ts";
import { readWorkspaceStartup, saveWorkspaceStartup, type WorkspaceStartup } from "./startup-preference.ts";

const workspaceIO = { readWorkspaceStartup, saveWorkspaceStartup, listStartupModels };

interface SettingsSource {
  readonly modelsPath?: string;
  readonly io?: typeof workspaceIO;
}

/** Owner-only callbacks. These are not tools and never change a live session's model. */
export function terminalSettings(role: HostIdentity["role"], root: string | undefined, corral: string,
  { modelsPath, io = workspaceIO }: SettingsSource = {}): SettingsControl {
  let startup: WorkspaceStartup | undefined;
  let bareModel: string | null | undefined;
  let generation = 0;
  const canSetStartup = role === "assistant" || role === "build";
  const personalStartup = root === undefined && role === "build";
  const choices = () => io.listStartupModels({ modelsPath });
  return {
    async read() {
      const reading = ++generation;
      const personal = readPersonalPreferences();
      let workspace: WorkspaceSettings;
      if (!canSetStartup) {
        workspace = { editable: false, reason: "This session's model is assigned by its profile." };
      } else if (personalStartup) {
        bareModel = personal.startupModel;
        workspace = { editable: true, name: "Personal · outside a workspace", startupModel: bareModel };
      } else if (!root || !corral) {
        workspace = { editable: false, reason: "No Corral workspace is connected." };
      } else {
        startup = undefined;
        try {
          const loaded = await io.readWorkspaceStartup(root, corral);
          if (reading === generation) startup = loaded;
          workspace = { editable: true, name: basename(root) || root, startupModel: loaded.model };
        } catch (error) {
          workspace = { editable: false, reason: error instanceof Error ? error.message : "Cannot read the workspace startup model." };
        }
      }
      return { personal, workspace };
    },
    listStartupModels: choices,
    async setStartupModel(selector) {
      // Capture the selection's revision before any await. Reopening Settings may fetch a newer one.
      const expected = startup, expectedBare = bareModel, saving = generation;
      if (!canSetStartup || (personalStartup ? expectedBare === undefined : !root || expected === undefined)) {
        throw new Error("The startup model is not editable here. Reopen Settings to try again.");
      }
      if (selector !== null && !(await choices()).some(model => model.selector === selector)) {
        throw new Error("This model is not available for new sessions here. Sign in or choose another model.");
      }
      if (personalStartup) {
        if (readPersonalPreferences().startupModel !== expectedBare) throw new Error("The startup model changed elsewhere; reopen Settings.");
        savePersonalPreference("startupModel", selector);
        if (saving === generation) bareModel = selector;
      } else if (root && expected) {
        const saved = await io.saveWorkspaceStartup(root, corral, expected, selector);
        if (saving === generation && startup === expected) startup = saved;
      }
    },
    async setPersonal(patch) {
      if (patch.appearance !== undefined) savePersonalPreference("appearance", patch.appearance);
      if (patch.density !== undefined) savePersonalPreference("density", patch.density);
      if (patch.motion !== undefined) savePersonalPreference("motion", patch.motion);
      if (patch.bell !== undefined) savePersonalPreference("bell", patch.bell);
    },
  };
}
