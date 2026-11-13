import { expect, test } from "bun:test";
import { KeybindingsManager } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { getKeybindings, setKeybindings, sliceByColumn, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { chromeTier } from "./header.ts";
import { contextHints, footerBase, footerRow, keyGroups, type HintState, type KeyContext } from "./keymap.ts";

const CONTEXTS: readonly KeyContext[] = ["composer", "agents", "agent", "changes", "task", "lifecycle", "proof", "artifacts",
  "inbox", "usage", "sidebar", "queue", "dialog", "palette", "keys", "pager"];
const state = (context: KeyContext, overrides: Partial<HintState> = {}): HintState => ({
  context, mode: { taskBound: true, answer: true, lifecycle: true }, streaming: true,
  queue: { items: 3, editable: 2, control: true },
  selection: ["open", "transcript", "steer", "cancel", "model"], authors: true, timelineSource: "task", ...overrides,
});

test("the footer ends with F1 and never overflows from 10 to 200 columns in every context", () => {
  for (const context of CONTEXTS) for (const selection of [undefined, ["open", "transcript"], ["output", "stop", "peek", "follow", "search", "background"]]) {
    const hints = contextHints(state(context, { selection }));
    const base = footerBase(context);
    for (let width = 10; width <= 200; width++) {
      const { line, hits } = footerRow(hints, chromeTier(width), width, base);
      expect(visibleWidth(line)).toBe(width);
      for (const hit of hits) expect(hit.to).toBeLessThanOrEqual(width);
      const ids = hits.map(hit => hit.hint.id);
      if (!base) { expect(ids).not.toContain("f1"); expect(ids).not.toContain("f2"); continue; }
      const last = hits[hits.length - 1]!;
      expect(last.hint.id).toBe("f1");
      // Nothing but F1's own label follows its cap.
      const tail = sliceByColumn(stripTerminalSequences(line), last.from, width - last.from).trim().split(/\s+/);
      expect(tail[0]).toBe("F1");
      expect(tail.slice(1).join(" ")).toBe(tail.length > 1 ? last.hint.label : "");
    }
  }
});

test("the follow-up and tool hints follow a changed user binding", () => {
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager({ "app.tools.expand": "alt+o", "app.message.followUp": "ctrl+j" }));
  try {
    const composer = contextHints(state("composer"));
    expect(composer.find(hint => hint.id === "followUp")?.key).toBe("Ctrl+J");
    const agent = contextHints(state("agent"));
    expect(agent.find(hint => hint.id === "tools")).toMatchObject({ key: "Alt+O", data: "\x1bo" });
    const row = footerRow(agent, "wide", 160, true);
    const tools = row.hits.find(hit => hit.hint.id === "tools")!;
    expect(tools.hint).toMatchObject({ key: "Alt+O", label: "tools" });
    expect(sliceByColumn(stripTerminalSequences(row.line), tools.from, tools.to - tools.from).trim().split(/\s+/)).toEqual(["Alt+O", "tools"]);
    const message = keyGroups({ taskBound: false, answer: false, jobs: true, queueControl: true, lifecycle: false })
      .find(group => group.title === "Message")!.rows.map(row => row.key);
    expect(message).toContain("Alt+O"); expect(message).toContain("Ctrl+J"); expect(message).not.toContain("Ctrl+O");
  } finally { setKeybindings(previous); }
});

test("the edit-queued hint appears only with an editable item and queue control", () => {
  const ids = (queue: HintState["queue"], streaming: boolean) =>
    contextHints(state("composer", { queue, streaming })).map(hint => hint.id);
  for (const streaming of [false, true]) {
    expect(ids({ items: 2, editable: 1, control: true }, streaming)).toContain("dequeue");
    expect(ids({ items: 2, editable: 0, control: true }, streaming)).not.toContain("dequeue");
    expect(ids({ items: 2, editable: 2, control: false }, streaming)).not.toContain("dequeue");
    expect(ids({ items: 0, editable: 0, control: true }, streaming)).not.toContain("dequeue");
  }
  expect(contextHints(state("composer", { streaming: false })).find(hint => hint.id === "dequeue")?.label).toBe("edit queued");
});
