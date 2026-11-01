import { expect, test } from "bun:test";
import { mkdtempSync, writeFileSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Check } from "typebox/value";
import { memoryContext } from "./memory.ts";
import { memoryTools } from "../roles/common/memory.ts";
import { defineCorralTool } from "../tools/define.ts";
import { CorralClient } from "../tools/corral.ts";
import worker from "../roles/worker/index.ts";
import pm from "../roles/pm/index.ts";
import assistant from "../roles/assistant/index.ts";
import reviewer from "../roles/reviewer/index.ts";
import planner from "../roles/planner/index.ts";
import planReviewer from "../roles/plan-reviewer/index.ts";
import { exploreLoadout } from "../roles/common/execution.ts";

test("only supported bound roles get authenticated memory tools; children get none", () => {
  for (const role of [worker, pm, assistant, reviewer, planner, planReviewer]) {
    const context = { identity: { store:"st-fixture", project:"demo", role:role.role, generation:1 }, cwd:"/tmp", model:"m" };
    const names = role.tools(context).filter(t => t.op.startsWith("memory_")).map(t => t.name);
    expect(names).toEqual(["worker","pm","assistant"].includes(role.role) ? memoryTools.map(t => t.name) : []);
    expect(exploreLoadout(names)).toEqual([]);
  }
});

test("memory tools validate selectors and CAS, and dispatch authenticated operations", async () => {
  const calls: unknown[] = [];
  const client = new CorralClient({ corral:"/unused", root:"/tmp",
    identity:{store:"st-fixture",project:"demo",role:"worker",generation:2},
    hostCap:"h".repeat(64), sessionId:() => "s-fixture" });
  client.call = async (op, call) => {
    const envelope = JSON.parse(client.envelope(op, call));
    calls.push(envelope);
    return { version:1, request_id:call.requestId, op, state:"committed", result:{} };
  };
  for (const spec of memoryTools) {
    const selector = {scope:{scope:"project",key:"demo"},id:"guide"};
    const payload = spec.op === "memory_list" ? {scope:selector.scope}
      : spec.op === "memory_put" ? {selector, expected_hash:null, entry:{
        metadata:{version:1,id:"guide",scope:selector.scope,title:"Guide",kind:"guidance",summary:"Keep evidence.",
          provenance:{kind:"agent_inference",sources:["task:T-1"]},updated_at:"2026-01-01T00:00:00Z",author:"placeholder"},body:"detail"}}
      : spec.op === "memory_remove" ? {selector,expected_hash:"a".repeat(64)} : {selector};
    expect(Check(spec.parameters, payload)).toBeTrue();
    expect(Check(spec.parameters, {...payload,root:"/other"})).toBeFalse();
    const tool = defineCorralTool(spec,{client,allowedOps:worker.corralOps,sessionId:() => "s-fixture",currentTurn:() => 3});
    await tool.execute("c-"+spec.op, payload, undefined, undefined, {} as never);
  }
  expect(calls).toHaveLength(4);
  for (const call of calls as {op:string;auth:{host_cap:string};identity:{generation:number;turn:number}}[]) {
    expect(call.op.startsWith("memory_")).toBeTrue();
    expect(call.auth.host_cap).toBe("h".repeat(64));
    expect(call.identity.generation).toBe(2);
    expect(call.identity.turn).toBe(3);
  }
});

test("generation context is bounded UTF-8 and immutable after its single read", () => {
  const dir = mkdtempSync(join(tmpdir(),"memory-host-"));
  const file = join(dir,"memory-context.md");
  try {
    writeFileSync(file,"# Retained memory\nProvenance: agent_inference\nSummary: café");
    for (const role of ["pm","assistant","worker"] as const) {
      expect(memoryContext(role,file,dir)).toContain("café");
    }
    expect(() => memoryContext("reviewer",file,dir)).toThrow("unavailable");
    writeFileSync(file,"é".repeat(6145));
    expect(() => memoryContext("worker",file,dir)).toThrow("12288");
    writeFileSync(file,Buffer.from([0xff]));
    expect(() => memoryContext("worker",file,dir)).toThrow();
    rmSync(file); symlinkSync("/dev/zero",file);
    expect(() => memoryContext("worker",file,dir)).toThrow();
  } finally { rmSync(dir,{recursive:true,force:true}); }
});
