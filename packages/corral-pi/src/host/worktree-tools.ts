import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { resolveToolPath } from "../roles/assistant/paths.ts";
import { classifyWriteScope } from "../roles/assistant/write-scope.ts";
import type { WorktreeAccess } from "./worktree-access.ts";
import type { WorktreeFiles } from "./worktree-files.ts";

/** Keep the SDK's exact edit semantics, rendering, abort checks and mutation queue. Swap in
 * anchored synchronous I/O only for the approved checkout, including calls from Code Mode. */
export function worktreeTools(sdk: Pick<typeof Sdk, "createEditToolDefinition" | "createWriteToolDefinition" | "defineTool">,
  cwd: string, options: {
    lock(path: string): Promise<{ check(): void; close(): void } | undefined>;
    takeover?: { scope: Parameters<typeof classifyWriteScope>[1]; access: WorktreeAccess;
      writesPlan(tool: string, input: { path: string }): boolean };
  }): ToolDefinition[] {
  const edit = sdk.createEditToolDefinition(cwd);
  const write = sdk.createWriteToolDefinition(cwd);
  return [sdk.defineTool({
    ...edit,
    execute: (...args: Parameters<typeof edit.execute>) => run(edit.name, args[1], args[2], args[4]?.cwd,
      () => edit.execute(...args), files => sdk.createEditToolDefinition(cwd, { operations: {
        access: async path => { files().access(path); },
        readFile: async path => files().read(path),
        writeFile: async (path, content) => { files().write(path, content, false); },
      } }).execute(...args)),
  }), sdk.defineTool({
    ...write,
    execute: (...args: Parameters<typeof write.execute>) => run(write.name, args[1], args[2], args[4]?.cwd,
      () => write.execute(...args), files => sdk.createWriteToolDefinition(cwd, { operations: {
        mkdir: async path => { files().mkdir(path); },
        writeFile: async (path, content) => { files().write(path, content, true); },
      } }).execute(...args)),
  })];

  async function run<T>(tool: string, input: { path: string }, signal: AbortSignal | undefined, effectiveCwd: string | undefined,
    ordinary: () => Promise<T>, operation: (files: () => WorktreeFiles) => Promise<T>): Promise<T> {
    const takeover = options.takeover;
    if (takeover && effectiveCwd && effectiveCwd !== cwd) throw new Error("File changes require the bound session directory.");
    const ordinaryWrite = async () => {
      if (takeover) {
        // A revoke between tool_call and execute must not fall through to unguarded SDK I/O.
        const decision = classifyWriteScope(input.path, takeover.scope);
        if (decision.action === "deny" && !takeover.writesPlan(tool, input)) throw new Error(decision.reason);
      }
      const lock = await options.lock(resolveToolPath(input.path, effectiveCwd || cwd));
      try {
        signal?.throwIfAborted();
        lock?.check();
        return await ordinary();
      } finally { lock?.close(); }
    };
    return takeover ? takeover.access.use(input.path, signal, operation, ordinaryWrite) : ordinaryWrite();
  }
}
