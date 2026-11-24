import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { BuildContext } from "../src/build/types.ts";
import role from "../src/roles/build/index.ts";
import type { RoleContext } from "../src/roles/types.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";

function context(trustedCode: unknown, workspace: boolean, project = workspace ? "sample" : "", cwd = "/repo"): RoleContext {
  return {
    identity: { store: "build", project, role: "build" },
    cwd, model: "test", todo: true,
    build: {
      settings: { maxParallel: 3, profiles: [], exitPolicy: "review" },
      session: { trustedCode, bare: !workspace, worktree: cwd, here: true,
        ...(workspace ? { workspaceRoot: cwd } : {}), ...(project ? { project } : {}) },
    } as unknown as BuildContext,
  };
}

async function attachArtifact(project: string) {
  const cwd = mkdtempSync(join(tmpdir(), "corral-code-artifact-"));
  try {
    const ctx = context(true, true, project, cwd);
    const engine = writeFakeEngine(cwd, { "artifact add": { result: { artifact: { task: "sample", id: 1, kind: "report" } } } });
    const client = new CorralClient({ corral: engine.path, root: cwd, cwd, identity: ctx.identity });
    const spec = role.tools(ctx).find(tool => tool.name === "corral_artifact");
    if (!spec) throw new Error("Code must offer corral_artifact");
    const tool = defineCorralTool(spec, { identity: ctx.identity, client, drained: async () => {}, todo() {}, checkpoint() {} });
    writeFileSync(join(cwd, "report.md"), "A report\n");
    const result = await tool.execute("attach", { path: "report.md", kind: "report" }, undefined, undefined, {} as never);
    return { result, calls: engine.calls().map(argv => argv.slice(3)) };
  } finally { rmSync(cwd, { recursive: true, force: true }); }
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

test("Code attaches reports to its bound project by default", async () => {
  const { result, calls } = await attachArtifact("sample");
  expect(result.isError).not.toBeTrue();
  expect(result.details).toMatchObject({ reference: "sample:1" });
  expect(calls).toEqual([["artifact", "add", "sample", "report.md", "--kind", "report"]]);
});

test("Code without a bound task or project must supply to", async () => {
  const { result, calls } = await attachArtifact("");
  expect(result.isError).toBeTrue();
  expect(result.content[0]).toMatchObject({ type: "text" });
  expect(JSON.stringify(result.details)).toContain("`to`");
  expect(calls).toEqual([]);
});

test("guarded Build and bare Code do not offer artifact attachment", () => {
  for (const ctx of [context(false, true), context(true, false)]) {
    expect(role.tools(ctx).map(tool => tool.name)).not.toContain("corral_artifact");
  }
});

test("only the literal trusted flag selects Code", () => {
  for (const flag of [undefined, false, "true", 1]) {
    const prompt = role.contract(context(flag, true));
    expect(prompt).toContain("Build contract");
    expect(prompt).toContain("Commit before build_verify");
    expect(prompt).not.toContain("Code contract");
  }
});
