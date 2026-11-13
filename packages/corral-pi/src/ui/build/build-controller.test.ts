import { expect, test } from "bun:test";
import type {
  BoardPlan, BoardPreview, BoardTaskStatus, BuildPanelState, BuildSummary, BuildUi, ChildSnapshot, ChildState, ExitChoice,
  ExitControl, ExitOption, ExitState,
} from "../../build/types.ts";
import { BuildController } from "./build-controller.ts";
import { BUILD_BOARD_ENTRY, BUILD_EXIT_ENTRY, BUILD_FINISH_ENTRY } from "./build-entries.ts";
import type { BuildEntry } from "./build-entries.ts";

const ENTER = "\r", ESC = "\x1b";
/** Detached host calls settle within a fixed number of microtask turns. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 20; turn++) await Promise.resolve();
}

const child = (id: string, state: ChildState, extra: Partial<ChildSnapshot> = {}): ChildSnapshot => ({
  id, call: "call-1", title: `${id} slice`, brief: "brief", owns: [], after: [], profile: "default", model: "anthropic/claude",
  state, createdAt: 0, edits: [], usage: { input: 0, output: 0, turns: 0 }, steers: [], pulse: [], history: "", messages: [], ...extra,
});
const asking = (id: string, askedAt: number) => child(id, "needs", {
  question: { question: "Which empty state?", askedAt, options: [{ label: "Show a new game action" }, { label: "Hide the picker" }] },
});
const OPTIONS: readonly ExitOption[] = [
  { choice: "keep", label: "Keep on the branch", detail: "keep it" },
  { choice: "land", label: "Land after the gate", detail: "land it", unavailable: "Project policy requires independent review." },
  { choice: "ship", label: "Ship through the board", detail: "ship it" },
  { choice: "iterate", label: "Keep iterating", detail: "stay" },
];
const PLAN: BoardPlan = {
  title: "Save slots", project: "mitiplay", why: "split",
  tasks: [
    { key: "manifest", title: "Slot manifest", depends: [], body: "b" },
    { key: "export", title: "Slot export", depends: ["manifest"], body: "b" },
  ],
};

function harness() {
  const s = {
    children: [] as ChildSnapshot[], target: "parent", exit: { phase: "none" } as ExitState, proposed: undefined as BoardPlan | undefined,
    tasks: [] as BoardTaskStatus[], steer: true, answer: true, draft: "", focus: "composer", calls: [] as string[],
    notices: [] as { text: string; tone: string }[], posts: [] as BuildEntry<unknown>[],
    preview: Promise.withResolvers<BoardPreview>(),
    /** Called where the host notifies exit subscribers and where the panels post an entry. */
    changed: undefined as (() => void) | undefined, posted: undefined as (() => void) | undefined,
  };
  const live = (id: string) => s.children.some(entry => entry.id === id && ["queued", "running", "needs"].includes(entry.state));
  const summary: BuildSummary = { branch: "build/x", base: "a", head: "b", commits: 2, files: 3, added: 10, removed: 1, dirty: false, slices: [], outside: [] };
  const exit: ExitControl & { open(): void } = {
    options: () => OPTIONS,
    summary: async () => summary,
    state: () => s.exit,
    choose: async (choice: ExitChoice) => {
      s.calls.push(`choose ${choice}`);
      s.exit = choice === "iterate" ? { phase: "none" } : { phase: "kept", record: { choice, at: "now" }, detail: "kept" };
      s.changed?.();
      return s.exit;
    },
    subscribe: () => () => {},
    open: () => { s.exit = { phase: "pending" }; s.changed?.(); },
  };
  const ui: BuildUi = {
    fleet: {
      snapshot: () => ({ children: s.children, maxParallel: 3 }),
      steer: async (id, text) => { s.calls.push(`steer ${id} ${text}`); return s.steer; },
      cancel: () => true,
      setModel: () => {},
      answer: (id, text) => { s.calls.push(`answer ${id} ${text}`); return s.answer; },
      models: () => [],
      profiles: () => [],
      subscribe: () => () => {},
    },
    exit,
    board: {
      tasks: () => s.tasks,
      proposed: () => s.proposed,
      preview: () => { s.calls.push("preview"); return s.preview.promise; },
      file: async (_plan, start) => {
        s.calls.push(`file ${start}`);
        await Promise.resolve();
        s.proposed = undefined;
        s.tasks = [{ id: "T-1", title: "Slot manifest", status: "Ready", phase: "queued", depends: [], updatedAt: 0 }];
        return s.tasks;
      },
      dismiss: () => { s.calls.push("dismiss"); s.proposed = undefined; },
      refresh: async () => {},
      subscribe: () => () => {},
    },
    target: () => s.target,
    setTarget: target => { s.target = target === "parent" || live(target) ? target : "parent"; },
  };
  const state = (): BuildPanelState => ({
    session: { version: 1, id: "B-1", name: "save-slots", created: "", updated: "", repoRoot: "/w/mitiplay", worktree: "/w/mitiplay/.worktrees/build-save-slots",
      branch: "build/save-slots", base: "a", here: false, project: "mitiplay", workspaceRoot: "/w" },
    settings: { exitPolicy: "gate", maxParallel: 3, profiles: [] },
    fleet: { children: s.children, maxParallel: 3 }, target: s.target, exit: s.exit, board: s.tasks,
  });
  const controller = new BuildController({
    ui, state, streaming: () => false, overlay: () => false,
    draft: () => s.draft, setDraft: text => { s.draft = text; },
    focusComposer: () => { s.focus = "composer"; }, focusDock: () => { s.focus = "choice"; },
    notice: (text, tone) => { s.notices.push({ text, tone }); },
    post: entry => { s.posts.push(entry); s.posted?.(); },
    showBoard: () => { s.calls.push("board"); },
    openChild: id => { s.calls.push(`open ${id}`); },
    color: () => "", now: () => 1000, changed: () => {},
  });
  return { s, controller, key: (data: string) => controller.dock.handleKey(data) };
}

test("a question that arrives while the composer holds a draft does not take focus", () => {
  const { s, controller } = harness();
  s.draft = "half-written";
  s.children = [asking("picker-ui", 10)];
  controller.sync();
  expect(controller.dialog()).toBeUndefined();
  expect(s.focus).toBe("composer");
  expect(controller.dock.focused).toBeFalse();
  expect(s.notices.some(notice => notice.tone === "warning")).toBeTrue();
  // Typing stays the composer's: the dock takes no keys while it is not focused.
  expect(controller.dock.handleKey("1")).toBeFalse();
  expect(s.calls).toEqual([]);
});

test("a question that arrives with an empty composer docks and takes focus", () => {
  const { s, controller, key } = harness();
  s.children = [asking("late", 20), asking("early", 10)];
  controller.sync();
  expect(controller.dialog()?.key).toBe("question:early:10");
  expect(s.focus).toBe("choice");
  expect(controller.dock.focused).toBeTrue();
  // The automatic open ignores digits for a moment.
  key("1");
  expect(s.calls).toEqual([]);
  controller.dock.guard(0);
  key("1");
  expect(s.calls).toEqual(["answer early Show a new game action"]);
});

test("Esc on the exit dialog keeps the phase pending and never chooses", () => {
  const { s, controller, key } = harness();
  s.exit = { phase: "pending" };
  controller.sync();
  expect(controller.dialog()?.key).toMatch(/^exit:/);
  key(ESC);
  expect(controller.dialog()).toBeUndefined();
  expect(s.exit.phase).toBe("pending");
  expect(s.calls).toEqual([]);
  expect(s.focus).toBe("composer");
  expect(controller.open("exit")).toBeTrue();
  expect(controller.dialog()).toBeDefined();
});

test("picking an unavailable exit never chooses and names the reason", async () => {
  const { s, controller, key } = harness();
  s.exit = { phase: "pending" };
  controller.sync();
  controller.dock.guard(0);
  key("2");
  key("j"); key("k"); key(ENTER);
  await settle();
  expect(s.calls).toEqual([]);
  expect(s.notices.filter(notice => notice.text.includes("Project policy requires independent review."))).toHaveLength(2);
  expect(controller.dialog()).toBeDefined();
});

test("filing waits for the preview and runs once even on a double pick", async () => {
  const { s, controller, key } = harness();
  s.proposed = PLAN;
  controller.sync();
  expect(s.calls).toEqual(["preview"]);
  controller.dock.guard(0);
  key("1");
  await settle();
  expect(s.calls).toEqual(["preview"]);
  s.preview.resolve({ tracker: "T-0", tasks: [{ key: "manifest", id: "T-1", title: "Slot manifest", depends: [] }, { key: "export", id: "T-2", title: "Slot export", depends: ["T-1"] }] });
  await settle();
  key("1"); key("1"); key(ENTER);
  await settle();
  controller.sync();
  expect(s.calls.filter(call => call.startsWith("file"))).toEqual(["file true"]);
  expect(s.posts.filter(entry => entry.customType === BUILD_BOARD_ENTRY)).toHaveLength(1);
  expect(s.calls).toContain("board");
  expect(controller.dialog()).toBeUndefined();
});

test("Not now discards the proposed plan", () => {
  const { s, controller, key } = harness();
  s.proposed = PLAN;
  controller.sync();
  controller.dock.guard(0);
  key("3");
  expect(s.calls).toContain("dismiss");
  expect(s.calls.some(call => call.startsWith("file"))).toBeFalse();
});

test("Enter with a child target steers it; a refusal keeps the draft; slash commands go to Pi", async () => {
  const { s, controller } = harness();
  s.children = [child("host-slots", "running")];
  expect(controller.point("host-slots")).toBeTrue();
  s.draft = "use the manifest lock";
  expect(await controller.submit("use the manifest lock")).toBeTrue();
  expect(s.calls).toEqual(["steer host-slots use the manifest lock"]);
  expect(s.draft).toBe("");

  s.steer = false;
  s.draft = "again";
  expect(await controller.submit("again")).toBeTrue();
  expect(s.draft).toBe("again");
  expect(s.notices.at(-1)?.tone).toBe("warning");

  expect(await controller.submit("/finish")).toBeFalse();
  expect(s.calls.filter(call => call.includes("/finish"))).toEqual([]);
  controller.reset();
  expect(await controller.submit("to the parent")).toBeFalse();
});

test("Alt+T skips queued and finished children and wraps to the parent", () => {
  const { s, controller } = harness();
  s.children = [child("queued", "queued"), child("one", "running"), child("done", "done"), asking("two", 5), child("gone", "cancelled"), child("three", "running")];
  const seen: string[] = [];
  for (let step = 0; step < 4; step++) { controller.cycleTarget(); seen.push(s.target); }
  expect(seen).toEqual(["one", "two", "three", "parent"]);

  const empty = harness();
  empty.s.children = [child("queued", "queued")];
  empty.controller.cycleTarget();
  expect(empty.s.target).toBe("parent");
  expect(empty.s.notices).toHaveLength(1);
});

test("answer mode routes Enter to the child's answer", async () => {
  const { s, controller, key } = harness();
  s.children = [asking("picker-ui", 10)];
  controller.sync();
  controller.dock.guard(0);
  key("3");
  expect(controller.target()).toEqual({ id: "picker-ui", answer: true });
  expect(s.focus).toBe("composer");
  s.draft = "Show both";
  expect(await controller.submit("Show both")).toBeTrue();
  expect(s.calls).toEqual(["answer picker-ui Show both"]);
  expect(s.calls.some(call => call.startsWith("steer"))).toBeFalse();
});

test("a settled exit posts one conversation entry", async () => {
  const { s, controller, key } = harness();
  s.exit = { phase: "pending" };
  controller.sync();
  controller.dock.guard(0);
  key("1");
  await settle();
  expect(s.calls).toEqual(["choose keep"]);
  controller.sync(); controller.sync();
  expect(s.posts.filter(entry => entry.customType === BUILD_EXIT_ENTRY)).toHaveLength(1);
});

test("one choice posts one exit entry, after earlier dismissals and with syncs from every host callback", async () => {
  const { s, controller, key } = harness();
  s.children = [child("alpha", "done"), child("beta", "done")];
  // The panels sync on every exit change and may sync again while an entry is posted.
  s.changed = () => controller.sync();
  s.posted = () => controller.sync();
  expect(controller.openFinish()).toBeTrue();
  for (const [width, rows, short] of [[140, 26, false], [80, 5, true], [52, 5, true]] as const) {
    controller.open("exit");
    controller.dock.setLimit(rows, short);
    controller.dock.render(width);
    key(ESC);
    expect(s.exit.phase).toBe("pending");
  }
  controller.open("exit");
  controller.dock.guard(0);
  key("1"); key("1"); key(ENTER);
  await settle();
  controller.sync(); controller.sync();
  expect(s.calls.filter(call => call.startsWith("choose"))).toEqual(["choose keep"]);
  expect(s.posts.filter(entry => entry.customType === BUILD_EXIT_ENTRY)).toHaveLength(1);
  expect(s.posts.filter(entry => entry.customType === BUILD_FINISH_ENTRY)).toHaveLength(1);
});
