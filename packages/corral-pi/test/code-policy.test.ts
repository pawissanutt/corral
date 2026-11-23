import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { BuildContext } from "../src/build/types.ts";
import { PLAN_ENTRY, PlanMode, type PlanEntry } from "../src/host/plan-mode.ts";
import { installToolControls } from "../src/host/tool-controls.ts";
import { buildParentPolicy, buildShellRefusal, classifyBuildWrite, installBuildGuard } from "../src/roles/build/guard.ts";
import type { RoleContext } from "../src/roles/types.ts";

function preflight(cwd: string, trustedCode?: true) {
  const build: BuildContext = {
    session: { version: 1, id: "B-test", created: "", updated: "", name: "test", here: true,
      repoRoot: cwd, worktree: cwd, branch: null, base: null, bare: true, trustedCode },
    settings: { exitPolicy: "none", maxParallel: 1, profiles: [] }, dir: cwd, corral: "corral",
  };
  const entries: PlanEntry[] = [];
  const plan = new PlanMode({
    dir: cwd, cwd, prompts: { contract: () => "", reset: () => "", approved: () => "" }, askUser: false, todo: false,
    entries: () => entries, leaf: () => String(entries.length),
    append: data => { entries.push({ type: "custom", customType: PLAN_ENTRY, data }); },
    busy: () => undefined, blocked: () => undefined, stopTurn: async () => {}, compact: async () => {},
    reset: async () => {}, submit: async () => true, model: () => undefined, changed() {}, note() {}, approved() {},
  });
  const calls: ((event: { toolName: string; input: object; toolCallId: string }) => Promise<unknown>)[] = [];
  const pi = { on(name: string, fn: typeof calls[number]) { if (name === "tool_call") calls.push(fn); } } as unknown as ExtensionAPI;
  installToolControls(pi, {
    ...buildParentPolicy({ build, cwd }, buildShellRefusal, { allow: [], deny: [], ask: [], default: "deny" }, plan),
    events: { emit() {} } as never, corral: new Set(), currentTurn: () => undefined,
  });
  installBuildGuard(pi, { build, cwd } as RoleContext);
  return { plan, async call(toolName: string, input: object) {
    for (const run of calls) {
      const refusal = await run({ toolName, input, toolCallId: "test" });
      if (refusal) return refusal;
    }
    return undefined;
  } };
}

test("only explicit Code parents bypass Build write and shell permissions, except while planning", async () => {
  const root = mkdtempSync(join(tmpdir(), "code-policy-"));
  try {
    const cwd = join(root, "checkout");
    mkdirSync(cwd);
    const outside = join(root, "outside.txt");
    const managed = preflight(cwd);
    const code = preflight(cwd, true);
    for (const [tool, input] of [
      ["write", { path: outside }], ["edit", { path: ".git/config" }],
      ["bash", { command: "git branch -D an-old-branch" }], ["job_start", { command: "corral list" }],
      ["bash", { command: "printf hello" }], ["other_command_tool", { command: "corral list" }],
    ] as const) {
      expect(await managed.call(tool, input)).toMatchObject({ block: true });
      expect(await code.call(tool, input)).toBeUndefined();
    }
    expect(classifyBuildWrite(outside, cwd).allowed).toBe(false);
    await code.plan.enter();
    expect(await code.call("write", { path: outside })).toMatchObject({ block: true });
    expect(await code.call("edit", { path: "source.txt" })).toMatchObject({ block: true });
    expect(await code.call("write", { path: code.plan.path })).toBeUndefined();
    expect(await code.call("bash", { command: "touch source.txt" })).toMatchObject({ block: true });
    expect(await code.call("job_start", { command: "touch source.txt" })).toMatchObject({ block: true });
  } finally {
    rmSync(root, { force: true, recursive: true });
  }
});
