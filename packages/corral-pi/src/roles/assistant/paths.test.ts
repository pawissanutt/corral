import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { PermissionBroker } from "./permissions.ts";
import { resolveToolPath, scratchRoots, writeRefusal } from "./paths.ts";

const snapshot = (session: string, paths: readonly string[]) => ({ session: { id: session, scratch: paths.map(path => ({ path })) } });

test("recorded scratch roots come only from the bound canonical session snapshot", () => {
  const workspace = "/workspace";
  const plain = join(workspace, ".worktrees", "A-1-plain");
  const quoted = join(workspace, ".worktrees", "A-1-it's");
  const data = snapshot("A-1", [plain, quoted, join(workspace, ".worktrees", "A-2-other"), join(workspace, "src")]);
  expect(scratchRoots(data, "A-1")).toEqual([plain, quoted]);
  expect(scratchRoots(data, "A-3")).toEqual([]);
  for (const invalid of [undefined, {}, { session: { id: "A-1", scratch: [{ path: null }, null] } }]) {
    expect(scratchRoots(invalid, "A-1")).toEqual([]);
  }
});

test("edit and write reach only the session's scratch, its artifact directory and the temp root", () => {
  const base = realpathSync(mkdtempSync(join(tmpdir(), "corral-assistant-paths-")));
  const workspace = join(base, "ws");
  const temp = join(base, "temp");
  const spike = join(workspace, ".worktrees", "A-1-spike");
  try {
    for (const dir of [spike, join(workspace, ".worktrees", "A-2-other"), join(workspace, ".worktrees", "T-9"), temp]) {
      mkdirSync(dir, { recursive: true });
    }
    writeFileSync(join(workspace, "README.md"), "main\n");
    symlinkSync(workspace, join(spike, "link"));
    let roots = scratchRoots(snapshot("A-1", [spike]), "A-1");
    const refused = (path: string) => writeRefusal(path, workspace, "A-1", temp, roots);
    const outside = [
      "README.md", join(workspace, "README.md"), ".worktrees/T-9/src/lib.rs", ".worktrees/A-2-other/x",
      ".worktrees/A-1-unrecorded/x", ".worktrees/A-1-spike/../../README.md", ".worktrees/A-1-spike/link/escape.txt",
      ".corral/assistants/A-1.md", ".corral/artifacts/assistant/A-2/draft.md", "~/notes.txt", join(base, "outside.txt"),
    ];
    expect(outside.map((path) => [path, refused(path)?.startsWith("assistant rule:") ?? false]))
      .toEqual(outside.map((path) => [path, true]));
    const inside = [
      ".worktrees/A-1-spike/src/new.rs", "@.worktrees/A-1-spike/notes.md", `file://${join(spike, "x.txt")}`,
      ".corral/artifacts/assistant/A-1/draft.md", join(temp, "scratch.txt"),
    ];
    expect(inside.map((path) => [path, refused(path)])).toEqual(inside.map((path) => [path, undefined]));
    const later = join(workspace, ".worktrees", "A-1-later");
    mkdirSync(later, { recursive: true });
    expect(refused(".worktrees/A-1-later/x")).toBeDefined();
    roots = scratchRoots(snapshot("A-1", [spike, later]), "A-1");
    expect(refused(".worktrees/A-1-later/x")).toBeUndefined();
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("the guard refreshes canonical scratch roots with permissions off and ignores a forged legacy mirror", async () => {
  const workspace = mkdtempSync(join(tmpdir(), "canonical-scratch-"));
  const scratch = join(workspace, ".worktrees/A-1-work");
  const ui = { hasUI: false, ui: { notify() {} } } as unknown as Pick<ExtensionContext, "hasUI" | "ui">;
  let roots: string[] = [];
  const broker = new PermissionBroker({ cwd: workspace, model: "fixture",
    identity: { store: "fixture", project: "", role: "assistant", assistant: "A-1", generation: 1 },
    scratchRoots: async () => roots,
  });
  try {
    mkdirSync(scratch, { recursive: true });
    mkdirSync(join(workspace, ".corral/assistants"), { recursive: true });
    writeFileSync(join(workspace, ".corral/assistants/A-1.md"), `---\nscratch:\n- path: ${scratch}\n---\n`);
    const write = () => broker.authorize("write", { path: join(scratch, "notes.md") }, "write", ui);
    expect((await write())?.block).toBeTrue();
    roots = [scratch];
    expect(await write()).toBeUndefined();
    roots = [];
    expect((await write())?.block).toBeTrue();
  } finally { rmSync(workspace, { recursive: true, force: true }); }
});

test("tool paths resolve like Pi's edit and write tools", () => {
  expect(resolveToolPath("@src/a.rs", "/w")).toBe("/w/src/a.rs");
  expect(resolveToolPath("/abs/../b", "/w")).toBe("/b");
  expect(resolveToolPath("a\u00A0b", "/w")).toBe("/w/a b");
});
