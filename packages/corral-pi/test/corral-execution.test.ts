import { expect, test } from "bun:test";
import { CORRAL_OPS } from "../src/roles/types.ts";
import { pmTools } from "../src/roles/pm/tools.ts";
import { executionRefusal, nestedCorralRead } from "../src/roles/common/execution.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { CorralClient } from "../src/tools/corral.ts";

const readOps = ["read_brief", "read_next", "read_show", "read_list", "read_search",
  "read_route", "read_artifacts", "read_lessons", "inbox_read"];

test("only receipt-free board reads permit nested execution; all other ops stay model-only", () => {
  expect(CORRAL_OPS.filter(nestedCorralRead)).toEqual(readOps);
  for (const op of CORRAL_OPS) {
    const name = `fixture_${op}`;
    const reads = new Set(readOps.includes(op) ? [name] : []);
    const corral = new Set([name]);
    expect(executionRefusal(name, true, new Set(), corral, reads)).toContain("loadout");
    expect(executionRefusal(name, false, corral, corral, reads)).toBeUndefined();
    const refusal = executionRefusal(name, true, corral, corral, reads);
    if (readOps.includes(op)) expect(refusal).toBeUndefined();
    else expect(refusal).toContain("directly");
  }
  for (const spec of pmTools) {
    const tool = defineCorralTool(spec, {
      allowedOps: CORRAL_OPS, sessionId: () => "bound-session", currentTurn: () => 1,
      client: new CorralClient({ corral: "unused", root: ".", hostCap: "unused", sessionId: () => "bound-session",
        identity: { store: "store", project: "project", role: "pm", generation: 1 } }),
    });
    expect(tool.exposure).toBe(readOps.includes(spec.op) ? "direct" : "model-only");
  }
});
