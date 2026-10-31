import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { resolveToolPath, scratchRoots, writeRefusal } from "./paths.ts";

/** A session record as the Rust store writes it: YAML frontmatter, then the Markdown body. */
function record(workspace: string, session: string, paths: readonly string[]): void {
  const scratch = paths.map((path, i) => `- slug: s${i}\n  project: corral\n  path: ${path}\n  branch: corral/${session}/s${i}\n`);
  mkdirSync(join(workspace, ".corral", "assistants"), { recursive: true });
  writeFileSync(join(workspace, ".corral", "assistants", `${session}.md`), [
    "---", `version: 1\nid: ${session}\nfocus: workspace\nprofile: pi`,
    `scratch:\n${scratch.join("")}artifacts:\n- run: ''\n  kind: diff\n  path: artifacts/assistant/${session}/abc-x.diff`,
    "---", "notes with path: /elsewhere\n",
  ].join("\n"));
}

test("recorded scratch paths are read from the frontmatter, quoted or plain, and nothing else", () => {
  const workspace = mkdtempSync(join(tmpdir(), "corral-assistant-roots-"));
  try {
    const plain = join(workspace, ".worktrees", "A-1-plain");
    record(workspace, "A-1", [plain, `'${join(workspace, ".worktrees", "A-1-it''s")}'`,
      JSON.stringify(join(workspace, ".worktrees", "A-1-json")), join(workspace, ".worktrees", "A-2-other"),
      join(workspace, "src")]);
    expect(scratchRoots(workspace, "A-1")).toEqual([
      plain, join(workspace, ".worktrees", "A-1-it's"), join(workspace, ".worktrees", "A-1-json"),
    ]);
    expect(scratchRoots(workspace, "A-3")).toEqual([]);
  } finally {
    rmSync(workspace, { recursive: true, force: true });
  }
});

test("edit and write reach only the session's scratch, its artifact directory and the temp root", () => {
  // Given A-1 with scratch spike, A-2's scratch, a task worktree and a link out of the scratch.
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
    record(workspace, "A-1", [spike]);
    const refused = (path: string) => writeRefusal(path, workspace, "A-1", temp);
    // Then the main checkout, task worktrees, other sessions and escapes are refused.
    const outside = [
      "README.md", join(workspace, "README.md"), ".worktrees/T-9/src/lib.rs", ".worktrees/A-2-other/x",
      ".worktrees/A-1-unrecorded/x", ".worktrees/A-1-spike/../../README.md", ".worktrees/A-1-spike/link/escape.txt",
      ".corral/assistants/A-1.md", ".corral/artifacts/assistant/A-2/draft.md", "~/notes.txt",
      join(base, "outside.txt"),
    ];
    const rule = "assistant rule: edit and write reach only this session's scratch";
    expect(outside.map((path) => [path, refused(path)?.startsWith(rule) ?? false]))
      .toEqual(outside.map((path) => [path, true]));
    // And the scratch (new subdirectories too), the session artifact directory and temp are allowed.
    const inside = [
      ".worktrees/A-1-spike/src/new.rs", "@.worktrees/A-1-spike/notes.md", `file://${join(spike, "x.txt")}`,
      ".corral/artifacts/assistant/A-1/draft.md", join(temp, "scratch.txt"),
    ];
    expect(inside.map((path) => [path, refused(path)])).toEqual(inside.map((path) => [path, undefined]));
    // A scratch made mid-session counts at once.
    const later = join(workspace, ".worktrees", "A-1-later");
    mkdirSync(later, { recursive: true });
    expect(refused(".worktrees/A-1-later/x")).toBeDefined();
    record(workspace, "A-1", [spike, later]);
    expect(refused(".worktrees/A-1-later/x")).toBeUndefined();
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("tool paths resolve like Pi's edit and write tools", () => {
  expect(resolveToolPath("@src/a.rs", "/w")).toBe("/w/src/a.rs");
  expect(resolveToolPath("/abs/../b", "/w")).toBe("/b");
  expect(resolveToolPath("a\u00A0b", "/w")).toBe("/w/a b");
});
