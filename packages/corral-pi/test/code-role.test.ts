import { expect, test } from "bun:test";
import type { BuildContext } from "../src/build/types.ts";
import role from "../src/roles/build/index.ts";
import type { RoleContext } from "../src/roles/types.ts";

function context(trustedCode: unknown, workspace: boolean): RoleContext {
  return {
    identity: { store: "/repo/.corral-next", project: "sample", role: "build" },
    cwd: "/repo", model: "test", todo: true,
    build: {
      settings: { maxParallel: 3, profiles: [], exitPolicy: "review" },
      session: { trustedCode, bare: !workspace, worktree: "/repo", here: true,
        ...(workspace ? { workspaceRoot: "/repo", project: "sample" } : {}) },
    } as unknown as BuildContext,
  };
}

test("Code retains Build tools and delegation without requiring the Build workflow", () => {
  const ctx = context(true, false);
  const prompt = role.contract(ctx);
  expect(role.upstreamTools).toEqual(["read", "edit", "write", "grep", "find", "ls", "bash"]);
  expect(role.tools(ctx).map(tool => tool.name)).toContain("corral_todo");
  expect(prompt).toContain("Code contract");
  expect(prompt).toContain("Parallel children");
  expect(prompt).toContain("No commit, gate, independent\nreview or `build_finish` call is required");
  expect(prompt).not.toMatch(/Exit policy:|Use build_finish|board_plan|<!--/);
});

test("workspace Code describes optional verification, review and board proposals", () => {
  const prompt = role.contract(context(true, true));
  expect(prompt).toContain("When available and useful, `build_verify`");
  expect(prompt).toContain("`build_review` reviews the committed diff");
  expect(prompt).toContain("`board_plan` only to propose follow-on work");
  expect(prompt).not.toContain("Commit before build_verify");
});

test("only the literal trusted flag selects Code", () => {
  for (const flag of [undefined, false, "true", 1]) {
    const prompt = role.contract(context(flag, true));
    expect(prompt).toContain("Build contract");
    expect(prompt).toContain("Commit before build_verify");
    expect(prompt).not.toContain("Code contract");
  }
});
