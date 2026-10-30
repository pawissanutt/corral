// Rich-mode projection of the same ledger used by Corral's board and CLI.
import type { ExtensionAPI, ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { z } from "zod";

import type { HostIdentity } from "../roles/types.ts";
import { ProgressSchema, TodoWidget, type TodoProgress } from "./todo-widget.ts";

const Receipt = z.object({
  state: z.literal("committed"),
  result: z.object({ progress: ProgressSchema }),
});
const WorkerSnapshot = z.object({
  runs: z.array(z.object({ id: z.string(), progress: ProgressSchema.nullable().optional() })),
});
const PmSnapshot = z.object({ container: z.object({ todo: ProgressSchema.nullable() }) });

export function snapshotProgress(value: unknown, identity: HostIdentity): TodoProgress | null {
  if (identity.role === "pm") return PmSnapshot.parse(value).container.todo;
  const run = WorkerSnapshot.parse(value).runs.find((run) => run.id === identity.run);
  if (!run) throw new Error(`todo_read: missing bound run ${identity.run}`);
  return run.progress ?? null;
}

export function installTodo(
  pi: ExtensionAPI, identity: HostIdentity, load: () => Promise<unknown>,
): void {
  let widget: TodoWidget | undefined;
  let ui: ExtensionUIContext | undefined;
  // A receipt arriving during a read is newer than that read's snapshot.
  let version = 0;
  const restore = async () => {
    const started = ++version;
    try {
      const progress = snapshotProgress(await load(), identity);
      if (version === started) widget?.update(progress);
    } catch (error) {
      if (version !== started) return;
      widget?.failed();
      ui?.notify(`Todo refresh failed: ${error instanceof Error ? error.message : String(error)}`, "warning");
    }
  };
  pi.on("session_start", async (_event, ctx) => {
    if (ctx.mode !== "tui") return;
    ui = ctx.ui;
    ui.setWidget("corral-todo", (tui, theme) => {
      widget = new TodoWidget(theme, () => tui.terminal.rows, () => tui.requestRender());
      return widget;
    }, { placement: "aboveEditor" });
    await restore();
  });
  pi.on("tool_result", async (event) => {
    if (!widget || event.isError || !["corral_todo", "corral_progress"].includes(event.toolName)) return;
    const receipt = Receipt.safeParse(event.details);
    if (receipt.success) {
      version++;
      widget.update(receipt.data.result.progress);
    } else if (z.object({ state: z.literal("committed") }).safeParse(event.details).success) {
      await restore();
    }
  });
  pi.registerCommand("todo", {
    description: "Toggle todo details; /todo up, down, or refresh",
    handler: async (args, ctx) => {
      if (!widget) {
        ctx.ui.notify("Todo visualization is available in the terminal UI.", "info");
        return;
      }
      switch (args.trim()) {
        case "": widget.toggle(); break;
        case "up": widget.scroll(-3); break;
        case "down": widget.scroll(3); break;
        case "refresh": await restore(); break;
        default: ctx.ui.notify("Use /todo, /todo up, /todo down, or /todo refresh.", "info");
      }
    },
  });
}
