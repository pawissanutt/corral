import { expect, test } from "bun:test";
import { executionRefusal } from "../src/roles/common/execution.ts";
import pm from "../src/roles/pm/index.ts";
import reviewer from "../src/roles/reviewer/index.ts";
import worker from "../src/roles/worker/index.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { CorralClient } from "../src/tools/corral.ts";

const roles = [worker, reviewer, pm];

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
        todo() { throw new Error("This test only checks the tool definition"); },
        checkpoint() { throw new Error("This test only checks the tool definition"); },
      });
      expect([tool.exposure, tool.description]).toEqual(["direct", spec.description]);
    }
  }
  expect(executionRefusal("codemode", false, new Set(["codemode"]))).toBeUndefined();
  expect(executionRefusal("codemode", true, new Set(["codemode"]))).toContain("codemode");
});
