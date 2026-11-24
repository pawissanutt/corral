// Each research session owns its kernel; nested calls are still checked by its loadout.
import { codemodeTool } from "../../capabilities/codemode.ts";
import { executionRefusal } from "../../roles/common/execution.ts";
import { CodeRuntime } from "./runtime.ts";

export function researchCode(allowed: ReadonlySet<string>) {
  const runtime = new CodeRuntime({ emit() {}, journal() {}, async notify() { return ""; } });
  return { runtime, tool: codemodeTool(runtime, names => names.filter(name => !executionRefusal(name, true, allowed)), true) };
}
