import { expect, test } from "bun:test";
import { KeybindingsManager } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { getKeybindings, setKeybindings, sliceByColumn, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { chromeTier } from "./header.ts";
import { contextHints, footerBase, footerRow, keyGroups, type HintState, type KeyContext } from "./keymap.ts";
import { PANEL_BLUE, PANEL_KEY } from "./panels-view.ts";

const CONTEXTS: readonly KeyContext[] = ["composer", "agents", "agent", "changes", "task", "lifecycle", "proof", "artifacts",
  "inbox", "usage", "image", "sidebar", "queue", "dialog", "palette", "keys", "pager", "board", "choice"];
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

test("while a turn runs the composer footer reads steer, queue, target, edit queued, then stop, as key caps", () => {
  const ids = contextHints(state("composer", { build: BUILD })).map(hint => hint.id);
  expect(ids.slice(0, 5)).toEqual(["submit", "followUp", "target", "dequeue", "interrupt"]);
  // The narrow tier keeps four: `edit queued` is one of them.
  const narrow = footerRow(contextHints(state("composer", { build: BUILD })), "narrow", 92, true);
  expect(narrow.hits.map(hit => hit.hint.id)).toContain("dequeue");
  const row = footerRow(contextHints(state("composer")), "wide", 160, true);
  expect(row.line).toContain(PANEL_KEY + PANEL_BLUE + " ⏎ ");
  expect(stripTerminalSequences(row.line).startsWith(" ")).toBeTrue();
});

test("as many hints as fit show at any width; F1 reads keys when none was dropped and more when one was", () => {
  const hints = contextHints(state("composer", { build: BUILD }));
  const ids = (width: number) => footerRow(hints, chromeTier(width), width, true).hits.map(hit => hit.hint.id);
  const label = (width: number) => footerRow(hints, chromeTier(width), width, true).hits.at(-1)!.hint.label;
  expect(ids(185)).toEqual([...hints.map(hint => hint.id), "f2", "f1"]); expect(label(185)).toBe("keys");
  // Narrow keeps every hint that fits, more than the old cap of four.
  expect(ids(92).length - 1).toBeGreaterThan(4);
  expect(label(46)).toBe("more"); expect(ids(46).length).toBeGreaterThan(1);
  const one = [contextHints(state("composer", { streaming: false, queue: { items: 0, editable: 0, control: true } }))[0]!];
  expect(footerRow(one, "narrow", 92, true).hits.map(hit => hit.hint.label)).toEqual(["send", "keys"]);
});

type Build = NonNullable<HintState["build"]>;
const BUILD: Build = { steerable: true, proposed: true, choices: 4 };
const TARGETS: readonly Build[] = [
  BUILD, { ...BUILD, steerable: false },
  { ...BUILD, target: { id: "a-child-with-a-rather-long-identifier", answer: false } },
  { ...BUILD, target: { id: "picker-ui", answer: true } },
];
const CHILD_ROW = ["open", "answer", "steer", "cancel", "model"];

test("the build footer never overflows from 10 to 200 columns, and the docked choice has no F2 or F1", () => {
  const cases: HintState[] = [
    ...TARGETS.flatMap(build => [false, true].map(streaming => state("composer", { build, streaming }))),
    state("agents", { build: BUILD, selection: CHILD_ROW }),
    state("agent", { build: BUILD, selection: CHILD_ROW }),
    state("board", { build: BUILD, selection: ["review", "details", "plan", "refresh"] }),
    state("board", { build: { ...BUILD, proposed: false }, selection: ["plan", "refresh"] }),
    state("choice", { build: BUILD }),
  ];
  for (const item of cases) {
    const hints = contextHints(item), base = footerBase(item.context);
    expect(hints.length).toBeGreaterThan(0);
    for (let width = 10; width <= 200; width++) {
      const { line, hits } = footerRow(hints, chromeTier(width), width, base);
      expect(visibleWidth(line)).toBe(width);
      for (const hit of hits) expect(hit.to).toBeLessThanOrEqual(width);
      const ids = hits.map(hit => hit.hint.id);
      if (item.context === "choice") { expect(ids).not.toContain("f1"); expect(ids).not.toContain("f2"); }
      else expect(ids[ids.length - 1]).toBe("f1");
    }
  }
  expect(footerBase("choice")).toBeFalse();
});

test("Alt+T appears only with a steerable child or a child target, and Enter names the child", () => {
  const target = (build: Build | undefined, streaming = false) => contextHints(state("composer", { build, streaming }));
  for (const streaming of [false, true]) {
    expect(target(BUILD, streaming).some(hint => hint.id === "target")).toBeTrue();
    expect(target({ ...BUILD, steerable: false }, streaming).some(hint => hint.id === "target")).toBeFalse();
    expect(target(undefined, streaming).some(hint => hint.id === "target")).toBeFalse();
  }
  const child = target({ ...BUILD, steerable: false, target: { id: "picker-ui", answer: false } });
  expect(child.some(hint => hint.id === "target")).toBeTrue();
  expect(child.find(hint => hint.id === "submit")?.label).toContain("picker-ui");
  // A child takes no queued follow-up, so the queue keys are not offered while it receives Enter.
  expect(child.some(hint => hint.id === "followUp" || hint.id === "dequeue")).toBeFalse();
  const answer = target({ ...BUILD, target: { id: "picker-ui", answer: true } });
  expect(answer.some(hint => hint.id === "target")).toBeTrue();
  expect(answer.find(hint => hint.id === "submit")?.label).toContain("picker-ui");
});

test("a answer is offered only when the selected child asks", () => {
  for (const context of ["agents", "agent"] as const) {
    expect(contextHints(state(context, { build: BUILD, selection: CHILD_ROW })).some(hint => hint.key === "a")).toBeTrue();
    expect(contextHints(state(context, { build: BUILD, selection: ["open", "steer", "cancel", "model"] })).some(hint => hint.key === "a")).toBeFalse();
  }
});

test("the board offers review, plan and task hints only when the view offers them", () => {
  const keys = (selection: string[]) => contextHints(state("board", { build: BUILD, selection })).map(hint => hint.key);
  expect(keys(["review", "details", "plan", "refresh"])).toEqual(["↑↓", "p", "⏎", "n", "r", "esc"]);
  expect(keys(["details"])).not.toContain("p");
  expect(keys(["details"])).not.toContain("n");
  // No task on the board: no task keys, only what still acts.
  expect(keys(["plan", "refresh"])).toEqual(["n", "r", "esc"]);
  // Outside a workspace with nothing filed: neither task keys nor planning.
  expect(keys([])).toEqual(["esc"]);
  const row = stripTerminalSequences(footerRow(contextHints(state("board", { build: BUILD, selection: [] })), chromeTier(140), 140, footerBase("board")).line);
  expect(row).not.toContain("↑↓"); expect(row).toMatch(/F2\s+actions/); expect(row).toMatch(/F1\s+keys/);
});

test("binding alt+t to an editor action removes Alt+T from the hints and the keys list", () => {
  const target = () => contextHints(state("composer", { build: BUILD, streaming: false })).some(hint => hint.id === "target");
  const listed = () => keyGroups({ taskBound: false, answer: false, jobs: false, queueControl: true, lifecycle: false, build: true })
    .flatMap(group => group.rows).some(row => row.key === "Alt+T");
  expect(target()).toBeTrue(); expect(listed()).toBeTrue();
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager({ "tui.editor.cursorWordLeft": "alt+t" }));
  try {
    expect(target()).toBeFalse(); expect(listed()).toBeFalse();
  } finally { setKeybindings(previous); }
});

test("the build keys list has Board and no Task, Proof or Inbox; other modes keep theirs", () => {
  const titles = (build: boolean, taskBound: boolean) =>
    keyGroups({ taskBound, answer: true, jobs: true, queueControl: true, lifecycle: true, build }).map(group => group.title);
  expect(titles(true, false)).toContain("Board");
  for (const title of ["Task", "Proof", "Inbox"]) expect(titles(true, false)).not.toContain(title);
  expect(titles(false, true)).toContain("Inbox"); expect(titles(false, true)).toContain("Proof");
  expect(titles(false, true)).not.toContain("Board");
});
