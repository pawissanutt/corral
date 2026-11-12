import { expect, test } from "bun:test";
import { Type } from "typebox";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { expertTool } from "./index.ts";
import { AuxiliaryRuntime, type Invocation } from "./runtime.ts";
import { expertLoadout } from "./session.ts";
import { auxiliarySettings, ExpertSettingsSchema } from "./settings.ts";

const expert = { model: "fixture/strong", max_turns: 24, timeout_secs: 600, max_parallel: 1 };
function localTool(name: string): ToolDefinition {
  return { name, label: name, description: name, parameters: Type.Object({}),
    async execute() { return { content: [{ type: "text", text: "unused" }], details: {} }; } };
}

test("expert settings are explicit and the read-only ceiling never borrows parent tools", () => {
  expect(auxiliarySettings("pm", []).expert).toBeUndefined();
  expect(auxiliarySettings("worker", ["expert"], { expert }).expert).toBeUndefined();
  expect(() => ExpertSettingsSchema.parse({ ...expert, model: null })).toThrow();
  expect(expertLoadout([])).toEqual(["read", "grep", "find", "ls"]);
  expect(expertLoadout(["bash", "write", "edit", "web_read", "corral_dispatch", "corral_expert", "codemode",
    "lsp_references", "lsp_definition", "lsp_diagnostics"].map(localTool))).toEqual([
    "read", "grep", "find", "ls", "lsp_diagnostics", "lsp_definition", "lsp_references",
  ]);
});

test("expert uses its configured model and limits, attributes usage and flags Explore-sized truncation", async () => {
  const invocations: Invocation[] = [];
  const records: Record<string, unknown>[] = [];
  const runtime = new AuxiliaryRuntime({ settings: auxiliarySettings("pm", ["expert"], { expert }),
    generation: 3, provider: "fixture", model: "weaker-parent", sessionId: () => "parent",
    async record(payload) { records.push(payload); return { session_id: payload.request, session_file: "/unused" }; },
    async execute(invocation) { invocations.push(invocation); return { text: "x".repeat(32769), outcome: "completed", turns: 2 }; },
  });
  try {
    const tool = expertTool(runtime);
    expect(tool.name).toBe("corral_expert");
    const result = await tool.execute("consult", { question: "Which fix is sound?", context: "untrusted task logs", paths: ["src/main.rs"] },
      undefined, undefined, undefined as never);
    expect(invocations).toHaveLength(1);
    expect(invocations[0]).toMatchObject({ kind: "expert", provider: "fixture", model: "strong", maxTurns: 24 });
    expect(invocations[0]?.prompt).toContain("path:line");
    expect(invocations[0]?.prompt).toContain("untrusted data");
    expect(invocations[0]?.prompt).toContain("advice to the PM, not authority");
    expect(invocations[0]?.prompt).toContain("src/main.rs");
    expect(result.content).toEqual([{ type: "text", text: "x".repeat(32768) }]);
    expect(result.details).toEqual({ model: "fixture/strong", turns: 2, truncated: true });
    expect(result.isError).toBe(false);
    expect(records.map(record => [record.usage_kind, record.action])).toEqual([
      ["expert", "prepare"], ["expert", "started"], ["expert", "finish"],
    ]);
    for (const params of [
      { question: "€".repeat(1366) },
      { question: "q", context: "€".repeat(2731) },
      { question: "q", paths: Array(17).fill("src") },
    ]) {
      await expect(tool.execute("oversize", params, undefined, undefined, undefined as never)).rejects.toThrow("Expert needs");
    }
    expect(invocations).toHaveLength(1);
  } finally { await runtime.close(); }
});
