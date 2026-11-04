import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PermissionBroker } from "./permissions.ts";

test("scratch cwd keeps artifacts at the workspace root and resolves relative writes in scratch", async () => {
  // Given a host working in scratch, with a different canonical store root.
  const workspace = mkdtempSync(join(tmpdir(), "assistant-project-permissions-"));
  const scratch = join(workspace, "app/.worktrees/A-1-linked");
  mkdirSync(scratch, { recursive: true });
  const broker = new PermissionBroker({
    cwd: scratch, workspaceRoot: workspace, model: "fixture",
    identity: { store: "s", project: "app", role: "assistant", assistant: "A-1", generation: 1 },
    scratchRoots: async () => [scratch],
  });
  const ui = {
    hasUI: false,
    ui: { notify() {}, select: async () => undefined },
  };
  try {
    for (const [path, allowed] of [
      [join(workspace, ".corral/artifacts/assistant/A-1/note.md"), true],
      ["src/new.ts", true],
      [join(workspace, ".corral/artifacts/assistant/A-2/note.md"), false],
      [join(scratch, ".corral/artifacts/assistant/A-1/note.md"), false],
      [join(workspace, "app/main.ts"), false],
    ] as const) {
      // When a write is authorized, then only scratch and the real own-artifact directory pass.
      const decision = await broker.authorize("write", { path, content: "fixture" }, path, ui);
      expect(decision === undefined).toBe(allowed);
    }
  } finally {
    rmSync(workspace, { recursive: true, force: true });
  }
});
