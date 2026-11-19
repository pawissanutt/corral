import { expect, test } from "bun:test";
import { RemoteRegistry } from "../src/host/remote/registry.ts";
import { contextPart, registerWorkspace, WorkspaceGit, type WorkspaceInputs } from "../src/host/remote/workspace.ts";
import { TodoLedger } from "../src/ui/todo.ts";

const context: WorkspaceInputs = { cwd: "/work", context_files: ["/work/AGENTS.md"], skills: [], prompts: [], tools: ["read"] };

test("the plan part reports a stale ledger instead of an empty plan", async () => {
  let fail = false;
  const ledger = new TodoLedger({ role: "assistant", project: "", store: "test" }, async () => {
    if (fail) throw new Error("read failed");
    return { container: { state: { todo: { plan: "P2", revision: 2, spec_revision: "test", phases: [{ id: "phase", label: "Do work",
      steps: [{ id: "s1", label: "Read source", status: { state: "in_progress" }, criteria: [] }] }] } } } };
  });
  const registry = new RemoteRegistry(), git = new WorkspaceGit(context.cwd, () => {});
  registerWorkspace(registry, { context: () => context, git, ledger: () => ledger,
    history: () => [{ at: "2026-01-01T00:00:00Z", revision: 1, summary: "P1" }] });
  try {
    await ledger.restore();
    const plan = registry.get("plan")!;
    const current = await plan({});
    expect(current).toMatchObject({ stale: false, plan: { revision: 2, steps: [{ id: "s1", state: "in_progress" }] } });
    fail = true;
    await expect(ledger.restore()).rejects.toThrow();
    expect(await plan({})).toEqual({ ...current, stale: true });
  } finally { git.close(); }
});

test("workspace context exposes MCP state without transport or error details", () => {
  const part = contextPart({ ...context, mcp: { servers: [{ name: "docs", connected: true, url: "http://private" },
    { name: "issues", connected: false, error: "secret config" }] } });
  expect(part.context_files).toEqual(context.context_files);
  expect(part.mcp).toEqual([{ name: "docs", state: "connected" }, { name: "issues", state: "unavailable" }]);
});
