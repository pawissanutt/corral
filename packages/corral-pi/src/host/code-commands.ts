import { statSync } from "node:fs";
import { resolve } from "node:path";
import type { ExtensionUIContext, SessionManager } from "@earendil-works/pi-coding-agent";
import { buildIsRunning } from "../build/cleanup.ts";
import { listSessions, loadSession } from "../build/session-store.ts";
import type { BuildSessionRecord } from "../build/types.ts";
import type { CodeHandoff, CodeHandoffRequest } from "./code-handoff.ts";
import { getCodeCloneSeed, getCodeForkChoices, getCodeForkSeed, readCodeImportSeed } from "./code-copies.ts";

interface Options {
  dataRoot: string;
  current: BuildSessionRecord;
  manager: () => SessionManager;
  ui: () => Pick<ExtensionUIContext, "select" | "confirm" | "notify">;
  busy: () => boolean;
  handoff: CodeHandoff;
}

export function createCodeCommands(options: Options): (text: string) => boolean {
  const { current, dataRoot } = options;
  let pending = false;
  const eligible = async (record: BuildSessionRecord) => record.trustedCode === true && record.id !== current.id
    && !record.cleaned && !!record.sessionFile && statSync(record.sessionFile, { throwIfNoEntry: false })?.isFile()
    && !(await buildIsRunning(dataRoot, record));
  return text => {
    const match = /^\/(new|resume|fork|clone|import)(?:\s+([\s\S]*))?$/.exec(text.trim());
    if (!match) return false;
    const ui = options.ui();
    if (pending || options.busy()) {
      ui.notify("Wait for the current Code action to finish before switching sessions.", "warning");
      return true;
    }
    pending = true;
    const [, cmd, arg] = match;
    // Let the terminal clear the submitted command before opening a picker.
    void Promise.resolve().then(async () => {
      let request: CodeHandoffRequest;
      if (cmd === "resume") {
        let id = arg;
        if (!id) {
          const choices = new Map<string, string>();
          for (const record of listSessions(dataRoot)) {
            if (await eligible(record).catch(() => false)) {
              const title = (record.title ?? record.name).replace(/[\x00-\x1f\x7f-\x9f]/g, " ").slice(0, 100);
              choices.set(`${record.id} · ${title}`, record.id);
            }
          }
          if (!choices.size) { ui.notify("No stopped Code sessions to resume.", "info"); return; }
          const selected = await ui.select("Resume a stopped Code session", [...choices.keys()]);
          id = selected === undefined ? undefined : choices.get(selected);
          if (!id) return;
        }
        // Re-read after selection; launch also verifies the target.
        if (!await eligible(loadSession(dataRoot, id))) throw new Error(`${id} is not an available stopped Code session.`);
        request = { kind: "resume", id };
      } else if (cmd === "import") {
        if (!arg) throw new Error("Usage: /import <path.jsonl>");
        const path = resolve(current.worktree, arg.replace(/^(['"])([\s\S]*)\1$/, "$2"));
        if (!await ui.confirm("Import into a new Code session?", path)) return;
        request = { kind: "new", seed: await readCodeImportSeed(path) };
      } else {
        if (arg) throw new Error(`Usage: /${cmd}`);
        if (cmd === "fork") {
          const manager = options.manager();
          const choices = getCodeForkChoices(manager);
          if (!choices.length) { ui.notify("No user messages to fork before.", "info"); return; }
          const labels = choices.map((choice, index) => `${index + 1}. ${choice.text.replace(/\s+/g, " ").slice(0, 120)}`);
          const selected = await ui.select("Fork conversation before this message", labels);
          const choice = selected === undefined ? undefined : choices[labels.indexOf(selected)];
          if (!choice) return;
          const { seed, selectedText } = getCodeForkSeed(manager, choice.id);
          request = { kind: "new", seed, draft: selectedText };
        } else if (cmd === "clone") {
          request = { kind: "new", seed: getCodeCloneSeed(options.manager()) };
        } else {
          request = { kind: "new" };
        }
      }
      if (options.busy()) throw new Error("Code became busy; wait for it to finish before switching sessions.");
      await options.handoff(request);
    }).catch((error: unknown) => ui.notify(error instanceof Error ? error.message : String(error), "error"))
      .finally(() => { pending = false; });
    return true;
  };
}
