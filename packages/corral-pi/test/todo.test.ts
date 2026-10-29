import { expect, test } from "bun:test";
import { Check } from "typebox/value";

import { todoTool } from "../src/roles/common/todo.ts";
import worker from "../src/roles/worker/index.ts";
import pm from "../src/roles/pm/index.ts";
import reviewer from "../src/roles/reviewer/index.ts";
import type { RoleModule } from "../src/roles/types.ts";

const step = { id: "implement", label: "Implement tool", criteria: ["AC-1"] };
const variants = [
  { op: "init", phases: [{ id: "build", label: "Build", steps: [step] }] },
  { op: "append", phase: "build", steps: [step, { id: "test", label: "Test tool" }] },
  { op: "start", step: "implement" },
  { op: "done", step: "implement" },
  { op: "drop", step: "implement", reason: "Superseded" },
];

for (const payload of variants) {
  test(`todo ${payload.op} validates and preserves its canonical payload`, () => {
    expect(Check(todoTool.parameters, payload)).toBeTrue();
    expect(todoTool.payload(payload)).toEqual(payload);
    for (const key of ["task", "run", "role", "revision", "spec_revision"]) {
      const foreign = { ...payload, [key]: "foreign" };
      expect(Check(todoTool.parameters, foreign)).toBeFalse();
      expect(todoTool.payload(foreign)).toEqual(payload);
    }
  });
}

test("todo rejects invalid variants, missing fields, and nested authority", () => {
  for (const payload of [
    {}, { op: "unknown" }, { op: "start" }, { op: "done", phase: "build" },
    { op: "drop", step: "implement" }, { op: "drop", step: "implement", reason: 1 },
    { op: "init", phases: [{ id: "build", label: "Build" }] },
    { op: "init", phases: [{ id: "build", label: "Build", steps: [], run: "R9" }] },
    { op: "append", phase: "build", steps: [{ ...step, task: "T-999" }] },
    { op: "append", phase: "build", steps: [{ ...step, criteria: [1] }] },
  ]) expect(Check(todoTool.parameters, payload)).toBeFalse();
});

for (const role of [worker, pm, reviewer] satisfies readonly RoleModule[]) {
  for (const todo of [undefined, false, true]) {
    test(`${role.role} todo=${String(todo)} selects only the launch-authorized loadout`, () => {
      const context = {
        identity: { store: "s", project: "demo", role: role.role, generation: 1 },
        cwd: "/tmp", model: "m", ...(todo === undefined ? {} : { todo }),
      };
      const tools = role.tools(context);
      const enabled = todo === true && role.role !== "reviewer";
      expect(tools.filter((tool) => tool.name === "corral_todo")).toEqual(enabled ? [todoTool] : []);
      expect(tools.every((tool) => role.corralOps.includes(tool.op))).toBeTrue();
    });
  }
}
