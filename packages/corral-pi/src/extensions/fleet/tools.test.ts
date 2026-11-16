import { expect, test } from "bun:test";
import type { ChildSpec } from "../../build/types.ts";
import type { FleetRuntime } from "./runtime.ts";
import { fleetTools } from "./tools.ts";

const spec: ChildSpec = { id: "child", title: "Child", brief: "Investigate", owns: [] };

test("roles without child permission retain only communication tools", () => {
  const runtime = {} as FleetRuntime;
  expect(fleetTools(runtime, []).map(tool => tool.name)).toEqual(["agent_message", "agent_roster"]);
  expect(fleetTools(runtime).map(tool => tool.name)).toEqual(["task", "agent_message", "agent_roster", "agent_wait", "agent_cancel"]);
});

test("task filters its schema and rejects disallowed kinds before starting any child", async () => {
  const calls: ChildSpec[][] = [];
  const runtime = { async start(_id: string, agents: ChildSpec[]) { calls.push(agents); return []; } } as unknown as FleetRuntime;
  const task = fleetTools(runtime, ["explore"]).find(tool => tool.name === "task")!;
  const schema = JSON.stringify(task.parameters);
  expect(schema).toContain('"explore"');
  expect(schema).not.toContain('"slice"');
  expect(schema).not.toContain('"review"');
  await task.execute("call", { agents: [spec] }, undefined, undefined, {} as never);
  expect(calls[0]![0]!.kind).toBe("explore");
  await expect(task.execute("call", { agents: [spec, { ...spec, kind: "slice" }] }, undefined, undefined, {} as never)).rejects.toThrow();
  expect(calls).toHaveLength(1);
  const defaultTask = fleetTools(runtime).find(tool => tool.name === "task")!;
  await defaultTask.execute("call", { agents: [spec] }, undefined, undefined, {} as never);
  expect(calls[1]![0]!.kind).toBe("slice");
});
