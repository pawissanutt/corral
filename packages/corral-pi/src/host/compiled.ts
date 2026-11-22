// Embedded SDK modules and assets for `bun build --compile src/cli.ts`.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import assistantContract from "../../assets/assistant-contract.md" with { type: "text" };
import gptAssistant from "../../assets/families/gpt/assistant.md" with { type: "text" };
import buildContract from "../../assets/build-contract.md" with { type: "text" };
import gptBuild from "../../assets/families/gpt/build.md" with { type: "text" };
import planSkill from "../../assets/skills/corral-plan/SKILL.md" with { type: "text" };
import planApproved from "../../assets/plan-approved.md" with { type: "text" };
import planMode from "../../assets/plan-mode.md" with { type: "text" };
import requestExecution from "../../assets/request-execution.md" with { type: "text" };
import requestExecutionOff from "../../assets/request-execution-off.md" with { type: "text" };
import reviewerContract from "../../assets/reviewer-contract.md" with { type: "text" };
import workerContract from "../../assets/worker-contract.md" with { type: "text" };
import workerSkill from "../roles/worker/skills/corral-worker/SKILL.md" with { type: "text" };
import { restrictEnvironment } from "./restrict.ts";
import { piDocs } from "./pi-docs-macro.ts" with { type: "macro" };

export const INSTALL_FILES: Readonly<Record<string, string>> = {
  "assets/assistant-contract.md": assistantContract,
  "assets/families/gpt/assistant.md": gptAssistant,
  "assets/build-contract.md": buildContract,
  "assets/families/gpt/build.md": gptBuild,
  "assets/skills/corral-plan/SKILL.md": planSkill,
  "assets/plan-approved.md": planApproved,
  "assets/plan-mode.md": planMode,
  "assets/request-execution.md": requestExecution,
  "assets/request-execution-off.md": requestExecutionOff,
  "assets/reviewer-contract.md": reviewerContract,
  "assets/worker-contract.md": workerContract,
  "src/roles/worker/skills/corral-worker/SKILL.md": workerSkill,
};

export function writeInstall(dir: string): void {
  for (const [relative, encoded] of Object.entries(piDocs())) {
    const path = join(dir, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, Buffer.from(encoded, "base64"), { mode: 0o644 });
  }
  for (const [relative, text] of Object.entries(INSTALL_FILES)) {
    const path = join(dir, relative);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text, { mode: 0o644 });
  }
}

async function installBundle(): Promise<void> {
  // Load role and SDK dependencies only after the executable's environment guard runs.
  const [{ setBundle }, worker, reviewer, assistant, build, auth] = await Promise.all([
    import("./bundle.ts"), import("../roles/worker/index.ts"), import("../roles/reviewer/index.ts"),
    import("../roles/assistant/index.ts"), import("../roles/build/index.ts"), import("../auth/index.ts"),
  ]);
  setBundle({
    roles: { worker: (root) => worker.workerRole(root), reviewer: (root) => reviewer.reviewerRole(root),
      assistant: (root) => assistant.assistantRole(root), build: (root) => build.buildRole(root) },
    auth: auth.default,
    writeAssets: writeInstall,
  });
}

export async function prepareCompiled(command: string): Promise<void> {
  restrictEnvironment(process.env);
  // Pi deliberately hides the optional OAuth implementation from normal imports.
  const { registerBunOAuthFlows } = await import("@earendil-works/pi-ai/bun-oauth");
  registerBunOAuthFlows();
  if (command === "run" || command === "chat" || command === "build") await installBundle();
}

