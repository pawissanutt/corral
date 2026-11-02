import { expect, test } from "bun:test";
import { CORRAL_OPS } from "../src/roles/types.ts";
import { pmTools } from "../src/roles/pm/tools.ts";
import { executionRefusal, nestedCorralRead } from "../src/roles/common/execution.ts";
import { defineCorralTool } from "../src/tools/define.ts";
import { CorralClient } from "../src/tools/corral.ts";

const readOps = ["read_brief", "read_next", "read_show", "read_list", "read_search",
  "read_route", "read_artifacts", "read_lessons", "inbox_read"];

test("one loadout check decides direct and Code Mode calls; every Corral tool is callable from code", () => {
  // The receipt-free reads remain the class whose failures need no receipt lookup.
  expect(CORRAL_OPS.filter(nestedCorralRead)).toEqual(readOps);
  for (const op of CORRAL_OPS) {
    const name = `fixture_${op}`;
    const loadout = new Set([name]);
    for (const nested of [false, true]) {
      expect(executionRefusal(name, nested, new Set())).toContain("loadout");
      expect(executionRefusal(name, nested, loadout)).toBeUndefined();
    }
    // A receipted call nested by anything but a Code Mode cell has no journal, so it is refused.
    expect(executionRefusal(name, true, loadout, true)).toContain("codemode cell");
    expect(executionRefusal(name, false, loadout, true)).toBeUndefined();
  }
  expect(executionRefusal("codemode", false, new Set(["codemode"]))).toBeUndefined();
  expect(executionRefusal("codemode", true, new Set(["codemode"]))).toContain("codemode");
  for (const spec of pmTools) {
    const tool = defineCorralTool(spec, {
      allowedOps: CORRAL_OPS, sessionId: () => "bound-session", currentTurn: () => 1,
      client: new CorralClient({ corral: "unused", root: ".", hostCap: "unused", sessionId: () => "bound-session",
        identity: { store: "store", project: "project", role: "pm", generation: 1 } }),
    });
    expect([tool.exposure, tool.description]).toEqual(["direct", spec.description]);
  }
});
