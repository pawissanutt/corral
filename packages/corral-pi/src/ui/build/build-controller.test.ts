import { expect, test } from "bun:test";
import { visibleWidth } from "@earendil-works/pi-tui";
import { BridgeRefusal } from "../../bridge/protocol.ts";
import type { BuildExitControl } from "../../build/exit.ts";
import type { BuildRecord } from "../../build/record-types.ts";
import type {
  BoardControl, BoardPlan, BoardPreview, BoardTaskStatus, BuildPanelState, BuildSummary, BuildUi, ChildSnapshot, ChildState, ExitChoice,
  ExitOption, ExitState,
} from "../../build/types.ts";
import { RemoteBuild } from "../../host/remote/build.ts";
import { RemoteDialogs } from "../../host/remote/dialogs.ts";
import { BuildController } from "./build-controller.ts";
import { BUILD_BOARD_ENTRY, BUILD_EXIT_ENTRY, BUILD_FINISH_ENTRY } from "./build-entries.ts";
import type { TranscriptMark } from "../../host/transcript-marks.ts";
import type { BuildEntry } from "./build-entries.ts";
import { choiceRows } from "./choice-dock.ts";
import { buildToolRenderers } from "./tool-renderers.ts";

const ENTER = "\r", ESC = "\x1b";
/** Detached host calls, and the registry's answers through the host's cards, settle within a fixed number of microtask turns. */
async function settle(): Promise<void> {
  for (let turn = 0; turn < 100; turn++) await Promise.resolve();
}

const child = (id: string, state: ChildState, extra: Partial<ChildSnapshot> = {}): ChildSnapshot => ({
  id, kind: "slice", call: "call-1", title: `${id} slice`, brief: "brief", owns: [], after: [], profile: "default", model: "anthropic/claude",
  state, createdAt: 0, edits: [], usage: { input: 0, output: 0, turns: 0 }, steers: [], pulse: [], history: "", messages: [], ...extra,
});
const asking = (id: string, askedAt: number) => child(id, "needs", {
  question: { question: "Which empty state?", askedAt, options: [{ label: "Show a new game action" }, { label: "Hide the picker" }] },
});
const OPTIONS: readonly ExitOption[] = [
  { choice: "keep", label: "Keep on the branch", detail: "keep it" },
  { choice: "land", label: "Land on main", detail: "land it", unavailable: "Project policy requires an approving review." },
  { choice: "iterate", label: "Keep iterating", detail: "stay" },
];
const USAGE = { input: 0, output: 0 };
const RECORD: BuildRecord = {
  version: 1, id: "B-1", kind: "build", title: "Save slots", intent: [], created: "", updated: "", repo_root: "/w/mitiplay",
  worktree: "/w/mitiplay/.worktrees/build-save-slots", branch: "build/save-slots", base: "a", outcome: { state: "open" }, commits: [], slices: [],
  verification: [], reviews: [], messages: [], decisions: [], usage: { parent: USAGE, children: USAGE, reviewers: USAGE, total: USAGE }, timeline: [],
  transcripts: { children: {} },
};
const PLAN: BoardPlan = {
  title: "Save slots", project: "mitiplay", why: "split",
  tasks: [
    { key: "manifest", title: "Slot manifest", depends: [], body: "b" },
    { key: "export", title: "Slot export", depends: ["manifest"], body: "b" },
  ],
};

/**
 * `remote`: the controller decides through a dialog registry over the host's own build cards (`RemoteBuild`), as in
 * an attached session; what the host posts and records lands in `hostPosts` and `marks`.
 */
function harness(options: { readonly remote?: boolean } = {}) {
  const s = {
    children: [] as ChildSnapshot[], target: "parent", exit: { phase: "none" } as ExitState, proposed: undefined as BoardPlan | undefined,
    tasks: [] as BoardTaskStatus[], steer: true, answer: true, draft: "", focus: "composer", calls: [] as string[], options: OPTIONS,
    notices: [] as { text: string; tone: string }[], posts: [] as BuildEntry<unknown>[], marks: [] as TranscriptMark[],
    hostPosts: [] as BuildEntry<unknown>[],
    summary: { branch: "build/x", base: "a", head: "b", commits: 2, files: 3, added: 10, removed: 1, dirty: false, slices: [], outside: [] } as BuildSummary,
    preview: Promise.withResolvers<BoardPreview>(),
    /** Called where the host notifies exit subscribers and where the panels post an entry. */
    changed: undefined as (() => void) | undefined, posted: undefined as (() => void) | undefined,
  };
  const live = (id: string) => s.children.some(entry => entry.id === id && ["queued", "running", "needs"].includes(entry.state));
  const exitListeners = new Set<() => void>();
  /** The exit's state changed: its subscribers hear it, as the host's exit control tells them. */
  const emit = () => {
    for (const listener of [...exitListeners]) listener();
    s.changed?.();
  };
  const exit: BuildExitControl = {
    options: () => s.options,
    summary: async () => s.summary,
    state: () => s.exit,
    choose: async (choice: ExitChoice, options?: { readonly overrideReason?: string; readonly authorization?: string }) => {
      s.calls.push(`choose ${choice}` + (options?.overrideReason === undefined ? "" : ` ${options.overrideReason}`)
        + (options?.authorization === undefined ? "" : ` @${options.authorization}`));
      s.exit = choice === "iterate" ? { phase: "none" } : choice === "land" ? { phase: "landed", record: { choice, at: "now", landed: "abc1234" } }
        : { phase: "kept", record: { choice, at: "now" }, detail: "kept" };
      emit();
      return s.exit;
    },
    subscribe: listener => { exitListeners.add(listener); return () => { exitListeners.delete(listener); }; },
    open: () => { s.exit = { phase: "pending" }; emit(); },
    close: () => {},
    busy: () => false,
    summarySnapshot: () => s.summary,
    // An explicit Finish opens the choice before its first await, as the host's does when nothing needs preparing.
    requestFinish: async () => { s.exit = { phase: "pending" }; emit(); },
    prepare: async () => undefined,
    preparationDetail: () => undefined,
    checkPrepared: async () => {},
    choiceBusy: () => false,
    choiceRevision: () => 0,
    gateProgress: () => {},
  };
  const board: BoardControl & { propose(plan: BoardPlan): void; close(): void } = {
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
    propose: plan => { s.proposed = plan; },
    close: () => {},
  };
  const ui: BuildUi = {
    fleet: {
      snapshot: () => ({ children: s.children, mail: [], maxParallel: 3 }),
      steer: async (id, text) => { s.calls.push(`steer ${id} ${text}`); return s.steer; },
      cancel: () => true,
      setModel: () => {},
      answer: (id, text) => { s.calls.push(`answer ${id} ${text}`); return s.answer; },
      models: () => [],
      profiles: () => [],
      message: async () => { throw new Error("Unused fixture messaging"); },
      roster: () => [],
      wait: async () => [],
      subscribe: () => () => {},
    },
    exit,
    record: () => RECORD,
    board,
    target: () => s.target,
    setTarget: target => { s.target = target === "parent" || live(target) ? target : "parent"; },
  };
  const state = (): BuildPanelState => ({
    session: { version: 1, id: "B-1", name: "save-slots", created: "", updated: "", repoRoot: "/w/mitiplay", worktree: "/w/mitiplay/.worktrees/build-save-slots",
      branch: "build/save-slots", base: "a", here: false, project: "mitiplay", workspaceRoot: "/w" },
    settings: { exitPolicy: "gate", maxParallel: 3, profiles: [] },
    fleet: { children: s.children, mail: [], maxParallel: 3 }, target: s.target, exit: s.exit, record: RECORD, board: s.tasks,
  });
  // The host's own cards for the exit, the plan and each child's question, listed as an attached session lists them.
  const build = options.remote ? new RemoteBuild({ exit, board, panelState: state }, {
    fleet: () => ui.fleet, children: () => s.children, ready: () => true, headless: () => false, working: () => false, changed: () => {},
    mark: mark => { s.marks.push(mark); },
    post: async entry => { s.hostPosts.push(entry); },
    error: text => { s.calls.push(`error ${text}`); },
  }) : undefined;
  const dialogs = build ? new RemoteDialogs(() => {}, { entries: () => build.entries(), absent: () => "missing", shared: () => true }) : undefined;
  const controller = new BuildController({
    ui, state, streaming: () => false, overlay: () => false,
    draft: () => s.draft, setDraft: text => { s.draft = text; },
    focusComposer: () => { s.focus = "composer"; }, focusDock: () => { s.focus = "choice"; },
    notice: (text, tone) => { s.notices.push({ text, tone }); },
    post: entry => { s.posts.push(entry); s.posted?.(); },
    marks: { add: mark => { s.marks.push(mark); return true; } },
    showPlan: () => { s.calls.push("board"); },
    openChild: id => { s.calls.push(`open ${id}`); },
    color: () => "", now: () => 1000, changed: () => {}, ...(dialogs ? { dialogs } : {}),
  });
  return { s, controller, dialogs, emit, exit, key: (data: string) => controller.dock.handleKey(data) };
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
  // Arrows move the selection away and back; a letter would be typing that hands the keys to the composer.
  key("\x1b[B"); key("\x1b[A"); key(ENTER);
  await settle();
  expect(s.calls).toEqual([]);
  expect(s.notices.filter(notice => notice.text.includes("Project policy requires an approving review."))).toHaveLength(2);
  expect(controller.dialog()).toBeDefined();
});

test("a Land that needs a reason shows it, takes the reason in the composer and lands only once one is given", async () => {
  const { s, controller, key } = harness();
  s.options = [OPTIONS[0]!, { choice: "land", label: "Land on main", detail: "land it", needsReason: "REVIEW_FINDINGS_OPEN" }, OPTIONS[2]!];
  s.exit = { phase: "pending" };
  controller.sync();
  const dialog = controller.dialog();
  expect(dialog?.options).toHaveLength(3);
  expect(dialog?.options[1]?.unavailable).toBeUndefined();
  expect(dialog?.options[1]?.detail).toContain("REVIEW_FINDINGS_OPEN");
  controller.dock.guard(0);
  key("2");
  await settle();
  expect(s.calls).toEqual([]);
  expect(controller.target()?.answer).toBeTrue();
  expect(s.focus).toBe("composer");
  // An empty reason does not land.
  expect(await controller.submit("   ")).toBeTrue();
  await settle();
  expect(s.calls).toEqual([]);
  s.draft = "Checked the finding by hand";
  expect(await controller.submit("Checked the finding by hand")).toBeTrue();
  await settle();
  expect(s.calls).toEqual(["choose land Checked the finding by hand"]);
  expect(s.draft).toBe("");
  expect(controller.target()).toBeUndefined();
});

const plain = (lines: readonly string[]) => lines.map(line => line.replace(/\x1b\[[0-9;]*m/g, "")).join("\n");
const TIP = "1111111aaaaaaa", HEAD = "2222222bbbbbbb";
const ready = (ticket: string, head = HEAD): { options: readonly ExitOption[]; exit: ExitState } => ({
  options: [OPTIONS[0]!, { choice: "land", label: "Land on main", detail: "Fast-forward main to the prepared candidate.", authorization: ticket,
    needsReason: "REVIEW_FINDINGS_OPEN" }, OPTIONS[2]!],
  exit: { phase: "pending", candidate: { state: "ready", base: "main", baseTip: TIP, head, gateAttempt: "G4" } },
});

test("a ready candidate names the exact fast-forward, and the reason lands the ticket the owner picked", async () => {
  const { s, controller, key } = harness();
  ({ options: s.options, exit: s.exit } = ready("T1"));
  controller.sync();
  const dialog = controller.dialog()!;
  expect(plain(dialog.body!(100, 4))).toContain("Land fast-forwards main 1111111 → 2222222 · gate G4 passed");
  expect(dialog.options[1]?.detail).toContain("fast-forward main 1111111 → 2222222");
  controller.dock.guard(0);
  key("2");
  expect(s.notices.at(-1)?.text).toContain("Enter lands (fast-forward main 1111111 → 2222222)");
  s.draft = "Checked by hand";
  expect(await controller.submit("Checked by hand")).toBeTrue();
  await settle();
  expect(s.calls).toEqual(["choose land Checked by hand @T1"]);
});

test("a candidate prepared again while the reason is typed is not landed by it", async () => {
  const { s, controller, key } = harness();
  ({ options: s.options, exit: s.exit } = ready("T1"));
  controller.sync();
  controller.dock.guard(0);
  key("2");
  ({ options: s.options, exit: s.exit } = ready("T2", "3333333ccccccc"));
  s.draft = "Checked by hand";
  expect(await controller.submit("Checked by hand")).toBeTrue();
  await settle();
  expect(s.calls).toEqual([]);
  expect(s.draft).toBe("Checked by hand");
  expect(s.notices.at(-1)?.text).toContain("The Land candidate changed");
  // The new candidate is shown before anything can land it.
  expect(plain(controller.dialog()!.body!(100, 4))).toContain("1111111 → 3333333");
});

test("a pick on a dialog shown with an older candidate shows the new one instead of landing", async () => {
  const { s, controller, key } = harness();
  ({ options: s.options, exit: s.exit } = ready("T1"));
  s.options = s.options.map(option => option.choice === "land" ? { ...option, needsReason: undefined } : option);
  controller.sync();
  controller.dock.guard(0);
  s.options = s.options.map(option => option.choice === "land" ? { ...option, authorization: "T2" } : option);
  key("2");
  await settle();
  expect(s.calls).toEqual([]);
  expect(s.notices.at(-1)?.text).toContain("The Land candidate changed");
  controller.dock.guard(0);
  key("2");
  await settle();
  expect(s.calls).toEqual(["choose land @T2"]);
});

test("an explicit Finish waits for the host to prepare, refuses a second Finish meanwhile, then docks the prepared choice once", async () => {
  const { s, controller, emit, exit } = harness();
  const ready = Promise.withResolvers<void>();
  let requests = 0;
  exit.requestFinish = async () => {
    requests++;
    s.exit = { phase: "rebasing", candidate: { state: "preparing", base: "main" } };
    emit();
    await ready.promise;
    s.exit = { phase: "pending", candidate: { state: "ready", base: "main", baseTip: TIP, head: HEAD } };
    emit();
  };
  expect(controller.openFinish()).toBeTrue();
  controller.sync();
  expect(controller.dialog()).toBeUndefined();
  // Preparing is not landing, and Finish waits for it.
  expect(controller.finishUnavailable()).toBe("the Land candidate is still being prepared");
  expect(controller.openFinish()).toBeFalse();
  ready.resolve();
  await settle();
  controller.sync();
  expect(requests).toBe(1);
  expect(plain(controller.dialog()!.body!(100, 4))).toContain("main 1111111 → 2222222");
  expect(s.posts.filter(entry => entry.customType === BUILD_FINISH_ENTRY)).toHaveLength(1);
});

test("conflicts read as conflicts, apart from a failed preparation; a host without a candidate claims none", () => {
  const { s, controller } = harness();
  s.options = [OPTIONS[0]!, { choice: "land", label: "Land on main", detail: "land it", unavailable: "Rebasing onto main conflicts." }, OPTIONS[2]!];
  s.exit = { phase: "pending", candidate: { state: "conflicts", base: "main", conflicts: ["src/a.ts", "src/b.ts"], detail: "Resolve them on the branch." } };
  controller.sync();
  const conflicts = plain(controller.dialog()!.body!(100, 6));
  expect(conflicts).toContain("Rebasing onto main conflicts; the branch is unchanged · 2 paths");
  expect(conflicts).toContain("│ src/a.ts");
  expect(conflicts).toContain("Resolve them on the branch.");
  expect(conflicts).not.toContain("failed");

  s.exit = { phase: "pending", candidate: { state: "failed", base: "main", detail: "git rebase exited 128" } };
  const failed = plain(controller.dialog()!.body!(100, 6));
  expect(failed).toContain("Preparing the Land candidate failed");
  expect(failed).not.toContain("conflicts");

  s.exit = { phase: "pending" };
  s.options = OPTIONS;
  const old = controller.dialog()!;
  expect(old.body).toBeUndefined();
  expect(old.options[1]?.detail).toBe("land it");
});

test("filing waits for the preview and runs once even on a double pick; the host posts the filed tasks", async () => {
  const { s, controller, key } = harness({ remote: true });
  const plan: BoardPlan = { ...PLAN, tasks: [
    { ...PLAN.tasks[0]!, title: "Long task title ".repeat(10), size: "s", difficulty: "hard", tags: ["Needs  Design", "Payments, EU"] },
    { ...PLAN.tasks[1]!, tags: ['quote" and \\slash', "Payments", "EU", "long tag ".repeat(45) + "end"] },
  ] };
  s.proposed = plan;
  controller.sync();
  expect(s.calls).toEqual(["preview"]);
  controller.dock.guard(0);
  key("1");
  await settle();
  expect(s.calls).toEqual(["preview"]);
  s.preview.resolve({ tracker: "T-0", tasks: plan.tasks.map((task, i) => ({ ...task, id: `T-${i + 1}` })) });
  await settle();

  controller.dock.setLimit(choiceRows(24, 9), true);
  const rows = controller.dock.render(80);
  const shown = plain(rows);
  expect(shown).toContain("Long task title");
  expect(shown).toContain('size: s · difficulty: hard · tags: ["Needs  Design","Payments, EU"]');
  expect(shown).toContain("more rows");
  expect(shown).toContain("in Chat");
  expect(rows.every(row => visibleWidth(row) <= 80)).toBeTrue();
  expect(controller.dialog()!.options[0]!.unavailable).toBeUndefined();

  // The live tool renderer reads the same accepted plan before any option is picked. Continuations keep every character.
  const renderers = buildToolRenderers("board_plan", () => controller)!;
  const result = renderers.renderResult!({ content: [], details: {} }, { expanded: false, isPartial: false }, undefined as never,
    { args: plan, state: {}, toolCallId: "plan", outputPad: 0, invalidate() {} } as never);
  const transcript = result.render(80);
  const text = plain(transcript);
  expect(text).toContain('tags: ["Needs  Design","Payments, EU"]');
  const exact = transcript.slice(transcript.findIndex(row => plain([row]).includes('tags: ["quote')));
  expect(exact.map(row => plain([row]).slice(2)).join("")).toBe(`tags: ${JSON.stringify(plan.tasks[1]!.tags)}`);
  expect(transcript.every(row => visibleWidth(row) <= 80)).toBeTrue();

  key("1"); key("1"); key(ENTER);
  await settle();
  controller.sync();
  expect(s.calls.filter(call => call.startsWith("file"))).toEqual(["file true"]);
  expect(s.hostPosts.filter(entry => entry.customType === BUILD_BOARD_ENTRY)).toHaveLength(1);
  expect(s.posts).toEqual([]);
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

test("one choice is recorded once, by the host, after earlier dismissals and with syncs from every host callback", async () => {
  const { s, controller, key } = harness({ remote: true });
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
  expect(s.hostPosts.filter(entry => entry.customType === BUILD_EXIT_ENTRY)).toHaveLength(1);
  expect(s.posts.filter(entry => entry.customType === BUILD_EXIT_ENTRY)).toEqual([]);
  expect(s.posts.filter(entry => entry.customType === BUILD_FINISH_ENTRY)).toHaveLength(1);
});

test("a keyboard decision on a watched card goes through the registry and is not applied twice", async () => {
  const { s, controller, dialogs, emit, key } = harness({ remote: true });
  const answered = () => s.marks.flatMap(mark => mark.kind === "child" && mark.act === "answered" ? [mark.text] : []);

  // The exit choice: one choose, one outcome entry (the host's), and the app's late answer is refused.
  s.exit = { phase: "pending" };
  emit();
  controller.sync();
  const exitCard = dialogs!.entries().find(entry => entry.card.kind === "exit")!.card.id;
  controller.dock.guard(0);
  key("1"); key("1"); key(ENTER);
  await settle();
  expect(s.calls).toEqual(["choose keep"]);
  let refusal = "";
  try { await dialogs!.resolve(exitCard, [{ options: ["keep"] }], false, { source: "remote" }); } catch (error) { refusal = error instanceof BridgeRefusal ? error.code : String(error); }
  expect(refusal).toBe("invalid_card_answer");
  expect(s.calls).toEqual(["choose keep"]);

  // A child's question, by its option and then in writing: each delivered and recorded once, by the host.
  s.children = [asking("picker-ui", 10)];
  controller.sync();
  controller.dock.guard(0);
  key("1"); key("1");
  await settle();
  s.children = [asking("picker-ui", 30)];
  controller.sync();
  controller.dock.guard(0);
  key("3");
  s.draft = "Show both";
  expect(await controller.submit("Show both")).toBeTrue();
  expect(s.draft).toBe("");
  expect(s.calls.filter(call => call.startsWith("answer"))).toEqual(["answer picker-ui Show a new game action", "answer picker-ui Show both"]);
  expect(answered()).toEqual(["Show a new game action", "Show both"]);

  // The board plan: filed once; the host posts the outcomes, the controller posts none.
  s.proposed = PLAN;
  controller.sync();
  s.preview.resolve({ tracker: "T-0", tasks: [{ key: "manifest", id: "T-1", title: "Slot manifest", depends: [] }] });
  await settle();
  controller.dock.guard(0);
  key("2"); key("2");
  await settle();
  expect(s.calls.filter(call => call.startsWith("file") || call === "dismiss")).toEqual(["file false"]);
  expect(s.hostPosts.map(entry => entry.customType)).toEqual([BUILD_EXIT_ENTRY, BUILD_BOARD_ENTRY]);
  expect(s.posts).toEqual([]);
});

test("hiding the dock with Esc leaves the decision pending", async () => {
  const { s, controller, dialogs, emit, key } = harness({ remote: true });
  s.exit = { phase: "pending" };
  emit();
  s.children = [asking("picker-ui", 10)];
  s.proposed = PLAN;
  controller.sync();
  for (const kind of ["exit", "question", "plan"] as const) {
    expect(controller.open(kind)).toBeTrue();
    key(ESC);
    expect(controller.dialog()).toBeUndefined();
  }
  await settle();
  expect(s.calls).toEqual(["preview"]);
  expect(dialogs!.entries().map(entry => entry.card.kind).sort()).toEqual(["board_plan", "child_question", "exit"]);
  // Still the app's to answer, once, and the hidden dock has nothing to say about it.
  const exitCard = dialogs!.entries().find(entry => entry.card.kind === "exit")!.card.id;
  expect(await dialogs!.resolve(exitCard, [{ options: ["keep"] }], false, { source: "remote", device: "phone" })).toBe("started");
  expect(s.calls).toEqual(["preview", "choose keep"]);
  expect(s.notices.some(notice => notice.text.includes("another device"))).toBeFalse();
});

test("an answer from the app closes the docked card with one quiet notice, and a later key press does nothing", async () => {
  const { s, controller, dialogs, key } = harness({ remote: true });
  s.children = [asking("picker-ui", 10)];
  controller.sync();
  expect(s.focus).toBe("choice");
  const card = dialogs!.entries().find(entry => entry.card.kind === "child_question")!.card;
  expect(await dialogs!.resolve(card.id, [{ options: [], text: "Show both" }], false, { source: "remote", device: "phone" })).toBe("delivered");
  expect(controller.dialog()).toBeUndefined();
  expect(s.focus).toBe("composer");
  expect(s.notices.filter(notice => notice.text.includes("another device"))).toEqual([{ text: "Answered from another device", tone: "text" }]);
  controller.dock.guard(0);
  key("1");
  await settle();
  expect(s.calls).toEqual(["answer picker-ui Show both"]);
});

test("Finish opens the chooser again after Iterate", async () => {
  const { s, controller, key } = harness({ remote: true });
  // Nothing built yet: the summary each episode reads is an empty branch, and no agent ever ran.
  s.summary = { ...s.summary, commits: 0, files: 0, added: 0, removed: 0 };
  s.changed = () => controller.sync();
  for (const episode of [1, 2, 3]) {
    expect(controller.openFinish()).toBeTrue();
    await settle();
    expect(controller.dialog()?.key).toBe(`exit:${episode}`);
    controller.dock.guard(0);
    key("3");
    await settle();
    expect(s.exit.phase).toBe("none");
    expect(controller.dialog()).toBeUndefined();
  }
  expect(s.calls).toEqual(["choose iterate", "choose iterate", "choose iterate"]);
  expect(s.hostPosts.filter(entry => entry.customType === BUILD_EXIT_ENTRY)).toHaveLength(3);
  expect(s.notices.filter(notice => notice.tone === "warning")).toEqual([]);
});

test("a child's question put aside in the app stays docked in the terminal and is still answered there", async () => {
  const { s, controller, dialogs, key } = harness({ remote: true });
  const phone = { source: "remote", device: "phone" } as const;
  const answered = () => s.marks.filter(mark => mark.kind === "child" && mark.act === "answered");
  const childCard = () => dialogs!.entries().find(entry => entry.card.kind === "child_question")!.card;
  s.children = [asking("picker-ui", 10)];
  controller.sync();
  expect(await dialogs!.resolve(childCard().id, [], true, phone)).toBe("dismissed");
  // Still docked, focused and silent: the child still waits.
  expect(controller.dialog()?.key).toBe("question:picker-ui:10");
  expect(s.focus).toBe("choice");
  expect(s.notices).toEqual([]);
  controller.dock.guard(0);
  key("1");
  await settle();
  expect(s.calls).toEqual(["answer picker-ui Show a new game action"]);
  expect(answered()).toHaveLength(1);
  expect(controller.dialog()).toBeUndefined();

  // Put aside, then answered in the app: the dock stays through the first and closes on the second, once.
  s.children = [asking("picker-ui", 30)];
  controller.sync();
  const later = childCard().id;
  expect(await dialogs!.resolve(later, [], true, phone)).toBe("dismissed");
  expect(controller.dialog()?.key).toBe("question:picker-ui:30");
  expect(await dialogs!.resolve(later, [{ options: [], text: "Show both" }], false, phone)).toBe("delivered");
  expect(controller.dialog()).toBeUndefined();
  expect(s.notices.filter(notice => notice.text.includes("another device"))).toEqual([{ text: "Answered from another device", tone: "text" }]);
  expect(s.calls).toEqual(["answer picker-ui Show a new game action", "answer picker-ui Show both"]);
  expect(answered()).toHaveLength(2);
});

test("a child's question, each answer and a delivered steer leave one transcript mark each", async () => {
  const { s, controller, key } = harness();
  const acts = () => s.marks.flatMap(mark => mark.kind === "child" ? [`${mark.act} ${mark.child}`] : []);
  s.children = [asking("picker-ui", 10)];
  controller.sync(); controller.sync();
  expect(acts()).toEqual(["asked picker-ui"]);
  controller.dock.guard(0);
  key("1");
  expect(acts()).toEqual(["asked picker-ui", "answered picker-ui"]);

  // A written answer through the composer.
  s.children = [asking("picker-ui", 30)];
  controller.sync();
  controller.dock.guard(0);
  key("3");
  s.draft = "Show both";
  await controller.submit("Show both");
  expect(acts().filter(act => act === "answered picker-ui")).toHaveLength(2);

  // A delivered steer names the child once; a refused one records nothing.
  s.children = [child("host-slots", "running")];
  controller.point("host-slots");
  await controller.submit("use the manifest lock");
  s.steer = false;
  await controller.submit("again");
  expect(acts().filter(act => act === "steered host-slots")).toHaveLength(1);
});

test("an exit choice or a plan that cannot dock over a draft still raises a notice", () => {
  const { s, controller } = harness();
  s.draft = "half-written";
  s.exit = { phase: "pending" };
  controller.sync();
  expect(controller.dialog()).toBeUndefined();
  expect(s.notices.filter(notice => notice.tone === "warning")).toHaveLength(1);
  s.proposed = PLAN;
  controller.sync();
  expect(controller.dialog()).toBeUndefined();
  expect(s.notices.filter(notice => notice.tone === "warning")).toHaveLength(2);
  // One that docks says nothing more.
  const docked = harness();
  docked.s.exit = { phase: "pending" };
  docked.controller.sync();
  expect(docked.controller.dialog()).toBeDefined();
  expect(docked.s.notices).toEqual([]);
});
