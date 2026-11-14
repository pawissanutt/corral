import { expect, test } from "bun:test";
import { KeybindingsManager } from "../../node_modules/@earendil-works/pi-coding-agent/dist/core/keybindings.js";
import { getKeybindings, setKeybindings, sliceByColumn, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { chromeTier } from "./header.ts";
import { contextHints, footerBase, footerRow, keyGroups, viewKey, type HintState, type KeyContext } from "./keymap.ts";
import { PANEL_BLUE, PANEL_KEY } from "./panels-view.ts";

const CONTEXTS: readonly KeyContext[] = ["composer", "agents", "agent", "changes", "task", "lifecycle", "proof", "artifacts",
  "inbox", "usage", "image", "sidebar", "queue", "dialog", "palette", "keys", "pager", "board", "choice", "questions"];
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

test("while a dialog is open the footer draws no cap: the dialog's own hint row is the one home of its keys", () => {
  expect(contextHints(state("dialog"))).toEqual([]);
  expect(footerRow(contextHints(state("dialog")), chromeTier(120), 120, footerBase("dialog")).hits).toEqual([]);
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

test("while a turn runs the composer footer reads steer, stop, queue, target, then edit queued, as key caps", () => {
  const ids = contextHints(state("composer", { build: BUILD })).map(hint => hint.id);
  expect(ids.slice(0, 5)).toEqual(["submit", "interrupt", "followUp", "target", "dequeue"]);
  // Stop survives the narrowest footers, with a queued item and a build receiver.
  for (const width of [46, 61]) expect(footerRow(contextHints(state("composer", { build: BUILD })), chromeTier(width), width, true).hits.map(hit => hit.hint.id)).toContain("interrupt");
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
  expect(footerRow(one, "narrow", 92, true).hits.map(hit => hit.hint.label)).toEqual(["send", "actions", "keys"]);
});

test("in Chat the composer footer names the tool-output key last, before F2 and F1, and drops it first when narrow", () => {
  const ids = (width: number, streaming: boolean) => footerRow(contextHints(state("composer", { chat: true, streaming, build: BUILD })), chromeTier(width), width, true)
    .hits.map(hit => hit.hint.id);
  for (const streaming of [false, true]) {
    const wide = ids(185, streaming);
    expect(wide.slice(-3)).toEqual(["tools", "f2", "f1"]);
    expect(ids(46, streaming)).not.toContain("tools");
  }
  const hint = contextHints(state("composer", { chat: true })).find(item => item.id === "tools");
  expect(hint).toMatchObject({ key: "Ctrl+O", label: "expand" });
  expect(contextHints(state("composer")).some(item => item.id === "tools")).toBeFalse();
  const message = keyGroups({ taskBound: false, answer: false, jobs: true, queueControl: true, lifecycle: false }).find(group => group.title === "Message")!;
  expect(message.rows.some(row => row.label === "Expand or collapse tool output")).toBeTrue();
});

type Build = NonNullable<HintState["build"]>;
const BUILD: Build = { steerable: true, proposed: true, choices: 4 };
const TARGETS: readonly Build[] = [
  BUILD, { ...BUILD, steerable: false },
  { ...BUILD, target: { id: "a-child-with-a-rather-long-identifier", answer: false } },
  { ...BUILD, target: { id: "picker-ui", answer: true } },
];
const CHILD_ROW = ["open", "answer", "steer", "cancel", "model"];

test("the build footer never overflows from 10 to 200 columns, and the docked choice keeps F2 and F1 when they fit", () => {
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
      expect(ids[ids.length - 1]).toBe("f1");
    }
  }
  expect(footerBase("choice")).toBeTrue();
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
  expect(keys(["review", "details", "plan", "refresh"])).toEqual(["↑↓", "p", "esc", "⏎", "n", "r"]);
  expect(keys(["details"])).not.toContain("p");
  expect(keys(["details"])).not.toContain("n");
  // No task on the board: no task keys, only what still acts.
  expect(keys(["plan", "refresh"])).toEqual(["n", "esc", "r"]);
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

const FOOTER_WIDTHS = [46, 61, 92, 104, 140];

test("F2 shows whenever the essential hints, F2 and F1 fit, at every width tier", () => {
  for (const context of CONTEXTS) {
    if (!footerBase(context)) continue;
    for (const width of FOOTER_WIDTHS) {
      const hints = contextHints(state(context, { streaming: false })), row = footerRow(hints, chromeTier(width), width, true);
      // Essential: the first hint, and Esc when it comes within the first three.
      const out = hints.slice(0, 3).findIndex(hint => hint.id === "escape"), kept = hints.slice(0, out < 0 ? 1 : out + 1);
      const gap = width < 64 ? 1 : 2;
      const minimum = kept.reduce((sum, hint) => sum + visibleWidth(hint.key) + 3 + visibleWidth(hint.short ?? hint.label), 0) + 12 + 9 + gap * (kept.length + 1);
      if (minimum <= width - 1) expect(row.hits.map(hit => hit.hint.id)).toContain("f2");
    }
  }
});

test("in the choice, agent, board, lifecycle and artifacts footers Esc comes right after the primary action and is never dropped", () => {
  const cases: HintState[] = [
    state("choice", { build: { ...BUILD, exit: true } }), state("agent", { selection: CHILD_ROW }),
    state("board", { build: BUILD, selection: ["review", "details", "plan", "refresh"] }), state("lifecycle"), state("artifacts"),
  ];
  for (const item of cases) {
    const hints = contextHints(item), at = hints.findIndex(hint => hint.id === "escape");
    expect(at).toBeGreaterThanOrEqual(0); expect(at).toBeLessThanOrEqual(2);
    for (const width of FOOTER_WIDTHS) expect(footerRow(hints, chromeTier(width), width, footerBase(item.context)).hits.map(hit => hit.hint.id)).toContain("escape");
  }
  // An exit choice offers Changes, after the way out.
  const exit = contextHints(state("choice", { build: { ...BUILD, exit: true } })).map(hint => hint.id);
  expect(exit.indexOf("f7")).toBeGreaterThan(exit.indexOf("escape"));
  expect(contextHints(state("choice", { build: BUILD })).some(hint => hint.id === "f7")).toBeFalse();
});

test("Usage advertises reread and token bars only when the view can act on them, never from the task binding alone", () => {
  const bound = { taskBound: true, answer: false, lifecycle: true };
  const keys = (usage: HintState["usage"]) => contextHints(state("usage", { mode: bound, ...(usage ? { usage } : {}) })).map(hint => hint.key);
  // A task-bound session without a collector reader, or one whose flags were never supplied, offers neither.
  for (const usage of [undefined, { reread: false, chart: false }]) {
    expect(keys(usage)).not.toContain("r"); expect(keys(usage)).not.toContain("c");
  }
  // A readable collector with an empty usage facet can be reread but has no token bars to draw.
  expect(keys({ reread: true, chart: false })).toContain("r"); expect(keys({ reread: true, chart: false })).not.toContain("c");
  expect(keys({ reread: true, chart: true })).toContain("c");
  for (const usage of [undefined, { reread: true, chart: false }, { reread: true, chart: true }]) {
    const hints = contextHints(state("usage", { mode: bound, ...(usage ? { usage } : {}) }));
    for (const width of FOOTER_WIDTHS) expect(footerRow(hints, chromeTier(width), width, true).hits.map(hit => hit.hint.id)).toContain("escape");
  }
});

test("a focused view's footer leads with who has the keys; a click on the lead replays Tab", () => {
  const hints = contextHints(state("changes", { lead: "Changes" }));
  expect(hints[0]).toMatchObject({ id: "lead", data: "\t" });
  expect(contextHints(state("changes")).some(hint => hint.id === "lead")).toBeFalse();
  const wide = footerRow(hints, chromeTier(140), 140, true), narrow = footerRow(hints, chromeTier(46), 46, true);
  expect(wide.hits[0]?.hint.id).toBe("lead"); expect(stripTerminalSequences(wide.line)).toContain("Tab to type");
  expect(narrow.hits[0]?.hint.id).toBe("lead"); expect(stripTerminalSequences(narrow.line)).toContain("Changes");
});

test("`/` is never a view key: filters and job search take F3", () => {
  for (const context of CONTEXTS) for (const selection of [undefined, ["output", "stop", "peek", "follow", "search", "background"]]) {
    expect(contextHints(state(context, { selection })).some(hint => hint.key === "/")).toBeFalse();
  }
  for (const context of ["changes", "inbox"] as const) expect(contextHints(state(context)).find(hint => hint.id === "filter")?.key).toBe("F3");
  expect(contextHints(state("agents", { selection: ["output", "search"] })).find(hint => hint.id === "search")?.key).toBe("F3");
  const keys = keyGroups({ taskBound: true, answer: true, jobs: true, queueControl: true, lifecycle: true }).flatMap(group => group.rows).map(row => row.key);
  expect(keys).not.toContain("/"); expect(keys).toContain("F3");
});

test("F1 lists each view's F-key with its Alt+digit, and the Scroll and Exit groups", () => {
  const groups = keyGroups({ taskBound: false, answer: true, jobs: false, queueControl: false, lifecycle: false });
  const views = groups.find(group => group.title === "Views")!.rows.map(row => row.key);
  expect(views).toEqual(["F9/Alt+1", "F8/Alt+2", "F7/Alt+3", "F11/Alt+4"]);
  for (const title of ["Scroll", "Exit"]) expect(groups.some(group => group.title === title)).toBeTrue();
  const exit = (build: boolean) => keyGroups({ taskBound: false, answer: false, jobs: false, queueControl: false, lifecycle: false, build })
    .find(group => group.title === "Exit")!.rows.map(row => row.key);
  expect(exit(true)).toContain("/finish"); expect(exit(false)).not.toContain("/finish");
  expect(exit(false)).toContain("Ctrl+D");
  // Exit rows are reference only: Enter in F1 never replays Ctrl+C or Ctrl+D.
  expect(groups.find(group => group.title === "Exit")!.rows.every(row => row.data === undefined)).toBeTrue();
  // A user binding on Alt+2 takes it away from the views.
  const previous = getKeybindings();
  setKeybindings(new KeybindingsManager({ "app.thinking.toggle": "alt+2" }));
  try { expect(viewKey(1)).toBeUndefined(); expect(viewKey(0)).toBe("alt+1"); } finally { setKeybindings(previous); }
});
