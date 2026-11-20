import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { classifyWriteScope } from "./write-scope.ts";

function fixture(run: (workspace: string, root: string) => void): void {
  const workspace = realpathSync(mkdtempSync(join(tmpdir(), "corral-write-scope-")));
  const root = join(workspace, ".worktrees", "build");
  try {
    mkdirSync(root, { recursive: true });
    run(workspace, root);
  } finally {
    rmSync(workspace, { recursive: true, force: true });
  }
}

test("write scope resolves from the tool cwd and asks for ordinary paths outside the root", () => fixture((workspace, root) => {
  const cwd = join(root, "src");
  mkdirSync(cwd);
  const scope = { cwd, root, workspace, buildWorktree: true };
  expect(classifyWriteScope("@a\u00A0b/new.ts", scope)).toEqual({ action: "allow" });
  const outside = join(workspace, "note.md");
  expect(classifyWriteScope(pathToFileURL(outside).href, scope)).toEqual({ action: "ask", path: outside, canonical: outside });
}));

test("write scope asks for ordinary symlink crossings in either direction", () => fixture((workspace, root) => {
  const outside = join(workspace, "other");
  const outbound = join(root, "outbound");
  const inbound = join(workspace, "inbound");
  mkdirSync(outside);
  symlinkSync(outside, outbound, "dir");
  symlinkSync(root, inbound, "dir");
  const scope = { cwd: root, root, buildWorktree: true };
  for (const [link, destination] of [[outbound, outside], [inbound, root]] as const) {
    const path = join(link, "new", "file.ts");
    expect(classifyWriteScope(path, scope)).toEqual({ action: "ask", path, canonical: join(destination, "new", "file.ts") });
  }
}));

test("write scope denies protected components on either side even outside the role root", () => fixture((workspace, root) => {
  const scope = { cwd: root, root, workspace, buildWorktree: true };
  for (const directory of [".git", ".corral", ".corral-next", ".worktrees"]) {
    expect(classifyWriteScope(join(workspace, "other", directory, "state"), scope).action).toBe("deny");
  }
  const state = join(workspace, ".corral");
  mkdirSync(state);
  symlinkSync(state, join(root, "alias"), "dir");
  symlinkSync(root, join(state, "alias"), "dir");
  expect(classifyWriteScope("alias/config", scope).action).toBe("deny");
  expect(classifyWriteScope(join(state, "alias", "file.ts"), scope).action).toBe("deny");
  expect(classifyWriteScope(join(workspace, ".worktrees", "sibling", "file.ts"), scope).action).toBe("deny");
}));

test("a starting directory does not exempt its protected ancestors", () => fixture((workspace, root) => {
  const state = join(workspace, ".corral-next");
  const fakeWorktree = join(state, ".worktrees", "nested");
  mkdirSync(fakeWorktree, { recursive: true });
  const alias = join(workspace, "state-alias");
  symlinkSync(state, alias, "dir");
  for (const cwd of [state, alias, root, fakeWorktree]) {
    expect(classifyWriteScope("corral.toml", { cwd, root: cwd, workspace }).action).toBe("deny");
  }
  expect(classifyWriteScope("config", { cwd: fakeWorktree, root: fakeWorktree, buildWorktree: true }).action).toBe("deny");
}));

test("write scope allows only bound Markdown memory with safe lexical and canonical suffixes", () => fixture((workspace, root) => {
  const memory = join(workspace, ".corral-next", "memory");
  mkdirSync(join(memory, ".git"), { recursive: true });
  const notes = join(memory, "notes.md");
  writeFileSync(notes, "");
  writeFileSync(join(memory, "data.json"), "{}");
  symlinkSync(join(memory, "data.json"), join(memory, "data.md"));
  symlinkSync(join(memory, ".git"), join(memory, "hidden"), "dir");
  symlinkSync(memory, join(root, "memory"), "dir");
  symlinkSync(root, join(memory, "escape"), "dir");
  const scope = { cwd: root, root, workspace };
  expect(classifyWriteScope(notes, scope)).toEqual({ action: "allow" });
  expect(classifyWriteScope(join(memory, "topics", "new.md"), scope)).toEqual({ action: "allow" });
  expect(classifyWriteScope(notes, { cwd: root, root }).action).toBe("deny");
  for (const path of ["data.json", "data.md", ".git/notes.md", "hidden/notes.md", "escape/notes.md"]) {
    expect(classifyWriteScope(join(memory, path), scope).action).toBe("deny");
  }
  expect(classifyWriteScope("memory/notes.md", scope).action).toBe("deny");
}));

test("write scope denies destinations that cannot resolve to regular files or new file locations", () => fixture((workspace, root) => {
  const scope = { cwd: root, root, buildWorktree: true };
  symlinkSync("./loop", join(root, "loop"));
  symlinkSync(join(workspace, "missing"), join(root, "dangling"));
  writeFileSync(join(root, "file"), "");
  for (const path of ["loop/new.ts", "dangling/new.ts", "file/new.ts", ".", "file:///%2Fbad", "null\0byte"]) {
    expect(classifyWriteScope(path, scope).action).toBe("deny");
  }
}));
