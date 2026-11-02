import { expect, test } from "bun:test";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { Text, stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { DecisionInbox, type InboxItem } from "./decision-inbox.ts";
import { InboxView } from "./inbox-view.ts";
import { NativeWorkbench } from "./workbench.ts";

const item: InboxItem = { kind: "question", task: "T-1", container: "T-1/R2", key: "T-1/R2/Q1", revision: "7", revision_kind: "question_revision", state: "open", pending: true, scope: "demo", reason: "Which route?\x1b]52;c;unsafe\x07", responsible: "pm_or_owner", detail: ["blocking=false"], inspect: "corral question show T-1/R2/Q1 --json", response: "Existing trusted owner answer flow", pm_eligible: true };
const read = async () => ({ version: 1, workspace: "/bound-workspace", observed_at: "2026-11-02T00:00:00Z", items: [item, { ...item, task: "T-2", key: "T-2/R1/Q1" }] });
const click = (y: number): TuiMouseEvent => ({ type: "click", button: "left", x: 1, y, screenX: 1, screenY: y, width: 80, height: 24, shift: false, alt: false, ctrl: false });

test("Inbox keyboard and mouse select exact records in the real main-view layout without replacing composer", async () => {
  const wb = new NativeWorkbench({ cwd: "/nonexistent", identity: { store: "s", project: "demo", role: "assistant", assistant: "A-1", generation: 1 }, snapshot: () => null, changed() {}, canInteract: () => true, readInbox: read });
  const root = wb.mount(new Text("REAL_TRANSCRIPT", 0, 0));
  try {
    expect(wb.handleInput("\x1b[23~")).toBeTrue();
    await wb.inbox.refresh();
    expect(wb.view).toBe("inbox");
    const screen = () => stripTerminalSequences(renderLayoutFrame(root, 80, 24, () => {}).lines.join("\n"));
    expect(screen()).toContain("READ ONLY"); expect(screen()).toContain("T-1/R2/Q1 @7");
    wb.handleInput("j"); expect(wb.inbox.current?.key).toBe("T-2/R1/Q1");
    wb.handleInput("\x1b[A"); expect(wb.inbox.current?.key).toBe("T-1/R2/Q1");
    wb.handleInput("\t"); expect(wb.focused).toBeFalse(); expect(wb.view).toBe("inbox");
    wb.handleInput("\x1b[20~"); expect(screen()).toContain("REAL_TRANSCRIPT");
  } finally { wb.dispose(); }
});

test("literal item hit rows and details are read-only, bounded and sanitized", async () => {
  const store = new DecisionInbox(read, () => {}); await store.refresh();
  let focused = 0, sheets = 0; const rendered: string[] = [];
  const ui = { async custom(factory: Parameters<ExtensionUIContext["custom"]>[0]) {
    sheets++;
    const component = await factory({ terminal: { rows: 34 }, requestRender() {} } as never, {} as never, {} as never, () => {});
    rendered.push(...component.render(80));
  }, notify() {} } as unknown as ExtensionUIContext;
  const view = new InboxView(store, "assistant", "", () => true, () => {}, () => { focused++; }); view.bind(ui);
  try {
    for (const width of [34, 52, 80, 140]) {
      for (const line of view.render(width)) {
        expect(visibleWidth(line)).toBeLessThanOrEqual(width);
        expect(line.replace(/\x1b\[[\d;]*m/g, "")).not.toMatch(/[\u0000-\u001f\u007f-\u009f]/);
      }
    }
    const row = view.rows.get("T-2/R1/Q1"); if (row === undefined) throw new Error("missing hit");
    expect(view.handleMouse(click(row))).toEqual({ handled: true }); expect(store.current?.key).toBe("T-2/R1/Q1");
    expect(focused).toBe(1); expect(sheets).toBe(0);
    await view.inspect(false); expect(sheets).toBe(1);
    expect(stripTerminalSequences(rendered.join("\n"))).toContain("READ ONLY");
    expect(stripTerminalSequences(rendered.join("\n"))).toContain("Nothing was sent or authorized");
    await view.inspect(true); expect(sheets).toBe(2);
    expect(stripTerminalSequences(rendered.join("\n"))).toContain("NOT SENT");
    await expect(store.answerFlow(item, "pm", "other")).rejects.toThrow("out_of_scope");
    expect(await store.answerFlow(item, "pm", "demo")).toContain("Existing trusted owner");
  } finally { view.dispose(); }
});
