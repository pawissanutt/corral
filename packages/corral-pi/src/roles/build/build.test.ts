import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { buildShellRefusal, classifyBuildWrite } from "./guard.ts";

test("build writes stay inside the session worktree and outside protected directories", () => {
  const base = realpathSync(mkdtempSync(join(tmpdir(), "corral-build-write-")));
  const worktree = join(base, ".worktrees", "build-test");
  mkdirSync(worktree, { recursive: true });
  try {
    expect(classifyBuildWrite(join(worktree, "src", "new.ts"), worktree)).toEqual({ allowed: true });
    expect(classifyBuildWrite("src/new.ts", worktree)).toEqual({ allowed: true });
    for (const path of [join(base, "outside.ts"), join(`${worktree}-other`, "file.ts"), "../outside.ts"]) {
      expect(classifyBuildWrite(path, worktree).allowed).toBe(false);
    }
    for (const directory of [".git", ".corral", ".worktrees"]) {
      expect(classifyBuildWrite(join(worktree, directory), worktree).allowed).toBe(false);
      expect(classifyBuildWrite(join(worktree, directory, "new"), worktree).allowed).toBe(false);
      expect(classifyBuildWrite(join(worktree, "nested", directory, "new"), worktree).allowed).toBe(false);
    }
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("build writes follow symlinks in existing ancestors, including protected destinations", () => {
  const base = realpathSync(mkdtempSync(join(tmpdir(), "corral-build-symlink-")));
  const worktree = join(base, "worktree");
  const outside = join(base, "outside");
  mkdirSync(join(worktree, "src"), { recursive: true });
  mkdirSync(join(worktree, ".git"));
  mkdirSync(outside);
  symlinkSync(outside, join(worktree, "escape"));
  symlinkSync(join(worktree, ".git"), join(worktree, "metadata"));
  symlinkSync(join(worktree, "src"), join(worktree, "source"));
  symlinkSync(worktree, join(base, "alias"));
  try {
    expect(classifyBuildWrite(join(worktree, "escape", "new", "file.ts"), worktree).allowed).toBe(false);
    expect(classifyBuildWrite(join(worktree, "metadata", "config"), worktree).allowed).toBe(false);
    expect(classifyBuildWrite(join(worktree, "source", "file.ts"), worktree)).toEqual({ allowed: true });
    expect(classifyBuildWrite(join(base, "alias", "src", "file.ts"), join(base, "alias"))).toEqual({ allowed: true });
  } finally {
    rmSync(base, { recursive: true, force: true });
  }
});

test("build shell refusals preserve fixed assistant rules and protect worktrees and branches", () => {
  for (const command of [
    "echo changed > .corral/state/session.json",
    "herdr pane send-text w1:p1 'hello'",
    "git worktree remove /repo/.worktrees/build-test",
    "git -C '/repo with spaces' worktree remove --force /repo/.worktrees/build-test",
    "git branch -D build/test",
    "git -C /repo branch -d build/test",
  ]) {
    expect([command, typeof buildShellRefusal(command)]).toEqual([command, "string"]);
  }
  for (const command of ["git status", "git commit -m 'Implement the slice'", "git -C /repo commit --only src/file.ts -m fix"]) {
    expect([command, buildShellRefusal(command)]).toEqual([command, undefined]);
  }
});
