import { expect, test } from "bun:test";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { executionRefusal } from "../src/roles/common/execution.ts";
import assistant from "../src/roles/assistant/index.ts";
import reviewer from "../src/roles/reviewer/index.ts";
import worker from "../src/roles/worker/index.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { CorralClient } from "../src/tools/corral.ts";
import { verdictTool } from "../src/roles/reviewer/tools.ts";
import { writeFakeEngine } from "./support/fake-engine.ts";

const roles = [worker, reviewer, assistant];

test("one loadout check decides direct and Code Mode calls; every Corral tool is callable from code", () => {
  for (const role of roles) {
    const identity = { store: "store", project: "project", role: role.role, task: "T-1", run: "R1" };
    const client = new CorralClient({ corral: "", root: ".", cwd: ".", identity });
    const specs = role.tools({ identity, cwd: ".", model: "fixture/model", todo: true });
    const loadout = new Set(specs.map((spec) => spec.name));
    for (const spec of specs) {
      for (const nested of [false, true]) {
        expect(executionRefusal(spec.name, nested, new Set())).toContain("loadout");
        expect(executionRefusal(spec.name, nested, loadout)).toBeUndefined();
      }
      const tool = defineCorralTool(spec, {
        identity, client,
        drained() { throw new Error("This test only checks the tool definition"); },
        todo() { throw new Error("This test only checks the tool definition"); },
        checkpoint() { throw new Error("This test only checks the tool definition"); },
      });
      expect([tool.exposure, tool.description]).toEqual(["direct", spec.description]);
    }
  }
  expect(executionRefusal("codemode", false, new Set(["codemode"]))).toBeUndefined();
  expect(executionRefusal("codemode", true, new Set(["codemode"]))).toContain("codemode");
});

test("a terminal verdict flushes reported usage before an engine that can close the reviewer", async () => {
  const root = mkdtempSync(join(tmpdir(), "corral-verdict-"));
  try {
    const engine = writeFakeEngine(root);
    const identity = { store: "local", project: "demo", role: "reviewer" as const, task: "T-1", run: "R2" };
    const client = new CorralClient({ corral: engine.path, root, cwd: root, identity });
    const usage = Promise.withResolvers<void>();
    const waiting = Promise.withResolvers<void>();
    const tool = defineCorralTool(verdictTool, {
      identity, client,
      drained() { waiting.resolve(); return usage.promise; },
      todo() { throw new Error("unexpected todo"); },
      checkpoint() { throw new Error("unexpected checkpoint"); },
    });
    const verdict = tool.execute("approve", { verdict: "approve", notes: "Looks good" }, undefined, undefined, {} as never);
    await Promise.race([waiting.promise, verdict]);
    expect(engine.calls()).toEqual([]);
    usage.resolve();
    await verdict;
    expect(engine.calls()).toEqual([["--root", root, "--json", "verdict", "T-1", "approve", "--notes", "Looks good"]]);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
