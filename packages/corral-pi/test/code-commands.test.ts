import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { InteractiveMode, SessionManager } from "@earendil-works/pi-coding-agent";
import { saveSession } from "../src/build/session-store.ts";
import type { BuildSessionRecord } from "../src/build/types.ts";
import { createCodeCommands } from "../src/host/code-commands.ts";
import type { CodeHandoffRequest } from "../src/host/code-handoff.ts";

async function fixture(run: (f: ReturnType<typeof setup>) => Promise<void>) {
  const dataRoot = mkdtempSync(join(tmpdir(), "corral-code-"));
  try { await run(setup(dataRoot)); }
  finally { rmSync(dataRoot, { recursive: true, force: true }); }
}

function setup(dataRoot: string) {
  const sessionFile = join(dataRoot, "transcript.jsonl");
  writeFileSync(sessionFile, "");
  const current: BuildSessionRecord = {
    version: 1, id: "B-current", name: "current", created: new Date().toISOString(), updated: new Date().toISOString(),
    repoRoot: dataRoot, worktree: dataRoot, branch: null, base: null, here: true, trustedCode: true, sessionFile,
  };
  const manager = SessionManager.inMemory();
  const requests: CodeHandoffRequest[] = [], notices: string[] = [], menus: string[][] = [];
  let busy = false;
  let pick: (choices: string[]) => Promise<string | undefined> = async () => undefined;
  const ui = {
    select: async (_title: string, choices: string[]) => { menus.push(choices); return pick(choices); },
    confirm: async () => false,
    notify: (text: string) => { notices.push(text); },
  };
  const command = createCodeCommands({ dataRoot, current, manager: () => manager, ui: () => ui,
    busy: () => busy, handoff: async request => { requests.push(request); } });
  return { save: (id: string, fields: Partial<BuildSessionRecord> = {}) => saveSession(dataRoot, { ...current, id, ...fields }),
    command, manager, requests, notices, menus, setBusy: (value: boolean) => { busy = value; },
    setPick: (next: typeof pick) => { pick = next; } };
}

const flush = () => new Promise<void>(resolve => setTimeout(resolve, 15));

test("terminal dispatch claims handoff commands before native Pi, and leaves ordinary input alone", async () => fixture(async f => {
  const cleared: string[] = [];
  const editor = { setText(text: string) { cleared.push(text); }, onSubmit: async (_text: string) => {} };
  let nativeCalls = 0;
  const fakeMode = { defaultEditor: editor, editor, options: { onTerminalCommand: f.command },
    showSettingsSelector() { nativeCalls++; } };
  const mode = InteractiveMode.prototype as unknown as { setupEditorSubmitHandler: (this: typeof fakeMode) => void };
  mode.setupEditorSubmitHandler.call(fakeMode);
  await editor.onSubmit(" /new ");
  await flush();
  await editor.onSubmit("/settings");
  expect(f.requests).toEqual([{ kind: "new" }]);
  expect(cleared).toEqual(["", ""]);
  expect(nativeCalls).toBe(1);
  expect(f.command("/newly")).toBe(false);
}));

test("resume picker lists stopped Code only; Esc cancels and explicit id bypasses picker", async () => fixture(async f => {
  f.save("B-current");
  f.save("B-stopped");
  f.save("B-build", { trustedCode: undefined });
  f.save("B-live", { pid: process.pid });
  f.save("B-nochat", { sessionFile: undefined });
  f.command("/resume");
  await flush();
  expect(f.menus).toEqual([["B-stopped · current"]]);
  expect(f.requests).toEqual([]);
  f.command("/resume B-stopped");
  await flush();
  expect(f.requests).toEqual([{ kind: "resume", id: "B-stopped" }]);
  expect(f.menus).toHaveLength(1);
  f.command("/resume B-build");
  await flush();
  expect(f.requests).toHaveLength(1);
  expect(f.notices.at(-1)).toContain("not an available stopped Code session");
}));

test("busy and overlapping actions are refused; picker rechecks busy and live target", async () => fixture(async f => {
  f.save("B-stopped");
  f.setBusy(true);
  f.command("/new");
  expect(f.notices.at(-1)).toContain("Wait");
  f.setBusy(false);
  f.setPick(async choices => { f.command("/new"); f.setBusy(true); return choices[0]; });
  f.command("/resume");
  await flush();
  expect(f.notices.at(-1)).toContain("became busy");
  expect(f.requests).toEqual([]);
  f.setBusy(false);
  f.setPick(async choices => { f.save("B-stopped", { pid: process.pid }); return choices[0]; });
  f.command("/resume");
  await flush();
  expect(f.requests).toEqual([]);
  expect(f.notices.at(-1)).toContain("not an available stopped Code session");
}));

test("clone and fork use new-record handoff; cancelled import leaves it alone", async () => fixture(async f => {
  f.manager.appendMessage({ role: "user", content: "first", timestamp: 1 });
  f.manager.appendMessage({ role: "user", content: "second", timestamp: 2 });
  f.command("/clone");
  await flush();
  f.setPick(async choices => choices[1]);
  f.command("/fork");
  await flush();
  f.command('/import "unused path.jsonl"');
  await flush();
  expect(f.requests).toMatchObject([{ kind: "new", seed: [{ type: "message" }, { type: "message" }] },
    { kind: "new", draft: "second", seed: [{ type: "message" }] }]);
}));
