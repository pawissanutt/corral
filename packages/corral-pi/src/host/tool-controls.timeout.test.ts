import { expect, test } from "bun:test";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { installToolControls } from "./tool-controls.ts";

test("short second-based timeouts are refused before execution", async () => {
  let handler: ((event: any) => Promise<any>) | undefined;
  const pi = { on(name: string, fn: any) { if (name === "tool_call") handler = fn; } } as unknown as ExtensionAPI;
  installToolControls(pi, { events: { emit() {} } as never, corral: new Set(), currentTurn: () => undefined });
  let spawned = false;
  const result = await handler!({ toolName: "bash", toolCallId: "1", input: { command: "touch sentinel", timeout: 0.6 } });
  if (!result?.block) spawned = true;
  expect(spawned).toBe(false);
  expect(result.reason).toContain("seconds");
});
