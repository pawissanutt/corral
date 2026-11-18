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
    for (const directory of [".git", ".corral", ".corral-next", ".worktrees"]) {
      expect(classifyBuildWrite(join(worktree, directory), worktree).allowed).toBe(false);
      expect(classifyBuildWrite(join(worktree, directory, "new"), worktree).allowed).toBe(false);
      expect(classifyBuildWrite(join(worktree, "nested", directory, "new"), worktree).allowed).toBe(false);
    }
    const memory = join(base, ".corral-next", "memory");
    mkdirSync(memory, { recursive: true });
    symlinkSync(worktree, join(memory, "escape"));
    expect(classifyBuildWrite(join(memory, "notes.md"), worktree, base)).toEqual({ allowed: true });
    expect(classifyBuildWrite(join(memory, "notes.md"), worktree).allowed).toBeFalse();
    expect(classifyBuildWrite(join(memory, "escape", "notes.md"), worktree, base).allowed).toBeFalse();
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

test("model shells cannot file board work or choose an exit through any corral invocation", () => {
  for (const command of [
    "corral --root /workspace import plan.md --origin owner --json",
    "corral --root='/workspace with spaces' new 'Unconfirmed work'",
    "corral list && corral move T-1 ready",
    "corral show T-1 || /usr/bin/corral approve T-1",
    "corral log T-1; ./target/debug/corral move T-1 ready",
    "printf plan | corral import plan.md --origin owner",
    "corral list\ncorral approve T-1",
    "printf '%s' \"$(corral --root /workspace import plan.md --origin owner)\"",
    "printf '%s' \"$(printf '%s' \"$(corral new nested)\")\"",
    "env -u AI_AGENT CORRAL_PM=0 /usr/bin/corral import plan.md --origin owner",
    "command -- corral import plan.md --origin owner",
    "exec -a owner ./target/debug/corral import plan.md --origin owner",
    "bash -lc 'corral list; corral import plan.md --origin owner'",
    "eval 'corral import plan.md --origin owner'",
    "corral search --reindex example",
    "corral route T-1 --clear-reviewer",
    "corral attention park T-1",
    "corral unknown-extension",
    "corral",
    '"$CORRAL" --json review T-1',
    "env -u CORRAL_ACTOR /opt/bin/corral-next --root=/workspace new 'Unconfirmed work'",
    "corral-next artifact add T-1 /tmp/evidence.log",
  ]) {
    const refusal = buildShellRefusal(command);
    expect([command, typeof refusal]).toEqual([command, "string"]);
    expect(refusal).toContain("/board");
    expect(refusal).toContain("/finish");
  }
  for (const command of [
    "corral --root '/workspace with spaces' show T-1 --json",
    "env -u AI_AGENT command corral --root=/workspace list --json",
    "corral log T-1 | cat",
    "printf '%s' \"$(/usr/bin/corral show T-1 --json)\"",
    "corral help import; corral --help; corral --version",
    "printf '%s' 'corral import plan.md --origin owner'",
    "echo '$(corral new example)'",
    "# corral import plan.md\ncorral list",
    '"${CORRAL}" --root /workspace --json show T-1',
    "corral-next artifact list T-1; corral-next artifact show T-1:1",
  ]) {
    expect([command, buildShellRefusal(command)]).toEqual([command, undefined]);
  }
  expect(buildShellRefusal("corral-next list", true)).toContain("bare session rule");
});
