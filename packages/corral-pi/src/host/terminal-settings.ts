import { basename } from "node:path";
import type { HostIdentity } from "../roles/types.ts";
import type { SettingsControl, WorkspaceSettings } from "../ui/settings-view.ts";
import { readPersonalPreferences, savePersonalPreference } from "./preferences.ts";
import { listStartupModels } from "./startup-model.ts";
import { readWorkspaceStartup, saveWorkspaceStartup, type WorkspaceStartup } from "./startup-preference.ts";

/** Owner-only callbacks. These are not tools and never change a live session's model. */
export function terminalSettings(role: HostIdentity["role"], root: string, corral: string): SettingsControl {
  let startup: WorkspaceStartup | undefined;
  let bareModel: string | null;
  const canSetStartup = role === "assistant" || role === "build";
  const bare = !corral && role === "build";
  return {
    async read() {
      const personal = readPersonalPreferences();
      let workspace: WorkspaceSettings;
      if (!canSetStartup) {
        workspace = { editable: false, reason: "This session's model is assigned by its profile." };
      } else if (bare) {
        bareModel = personal.startupModel;
        workspace = { editable: true, name: "Personal · outside a workspace", startupModel: bareModel };
      } else if (!corral) {
        workspace = { editable: false, reason: "No Corral workspace is connected." };
      } else {
        startup = undefined;
        try {
          startup = await readWorkspaceStartup(root, corral);
          workspace = { editable: true, name: basename(root) || root, startupModel: startup.model };
        } catch (error) {
          workspace = { editable: false, reason: error instanceof Error ? error.message : "Cannot read the workspace startup model." };
        }
      }
      return { personal, workspace };
    },
    listStartupModels: () => listStartupModels(),
    async setStartupModel(selector) {
      if (!canSetStartup || (!bare && !startup)) throw new Error("The startup model is not editable here. Reopen Settings to try again.");
      if (selector !== null && !(await listStartupModels()).some(model => model.selector === selector)) {
        throw new Error("This model is not available for both new chats and Builds. Sign in or choose another model.");
      }
      if (bare) {
        if (readPersonalPreferences().startupModel !== bareModel) throw new Error("The startup model changed elsewhere; reopen Settings.");
        savePersonalPreference("startupModel", selector);
        bareModel = selector;
      } else if (startup) {
        startup = await saveWorkspaceStartup(root, corral, startup, selector);
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
