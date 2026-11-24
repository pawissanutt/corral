import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { TuiAltScreen, stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { AgentConversation, conversationHeader, type AgentConversationHost } from "./agent-conversation.ts";
import type { TranscriptRead } from "./agent-history.ts";
import { helperRows, type AgentRow } from "./agents-model.ts";
import { agentColor } from "./agent-hues.ts";
import { HelperChat } from "./helper-chat.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";
import type { ChildSnapshot } from "../build/types.ts";
import { toolMessageId } from "../extensions/subagent/inspection.ts";
import type { TranscriptPage, TranscriptRow } from "../host/transcript-preview.ts";
import { childRow } from "./build/fleet-rows.ts";

beforeAll(() => initTheme("dark", false));
const NOW = 1_000_000;
const base: PanelSnapshot = { bound: true, model: "m", effort: "off", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null, jobs: [] };
function helper(extra: Partial<HelperInspection>): HelperInspection {
  return { request: "r", kind: "explore", model: "provider/model-a", phase: "running", createdAt: NOW - 9000, startedAt: NOW - 8000,
    prompt: "Question", messages: [{ id: "a", kind: "assistant", text: "Visible response" }], truncated: false, ...extra };
}
function rowOf(entry: HelperInspection): AgentRow {
  const [row] = helperRows({ ...base, helpers: [entry] }, NOW);
  if (!row) throw new Error("no helper row");
  return row;
}
function conversation(row: () => AgentRow | undefined, chat: HelperChat, back: () => void = () => {}): AgentConversation {
  return new AgentConversation({ row, chat, color: agentColor, now: () => NOW,
    click: (event: TuiMouseEvent) => event.type === "click" && event.button === "left", focus() {}, back });
}

test("the returned result is shown once, and observed errors stay visible", () => {
  const result = "\n".repeat(150) + "FINAL_RESULT_VISIBLE";
  const appended = rowOf(helper({ phase: "completed", endedAt: NOW - 1000, result, error: "OBSERVED_ERROR_VISIBLE", inspectionError: "INSPECTOR_WARNING_VISIBLE",
    messages: Array.from({ length: 8 }, (_, index) => ({ id: String(index), kind: "assistant" as const, text: "line\n".repeat(40) })) }));
  const answered = rowOf(helper({ phase: "completed", endedAt: NOW - 1000, result: "FINAL_RESULT_VISIBLE",
    messages: [{ id: "a", kind: "assistant", text: "FINAL_RESULT_VISIBLE", stopReason: "stop" }] }));
  for (const row of [appended, answered]) {
    const text = stripTerminalSequences(conversation(() => row, new HelperChat(() => undefined, "/tmp")).scroll.render(80).join("\n"));
    expect(text.split("FINAL_RESULT_VISIBLE").length - 1).toBe(1);
  }
  const text = stripTerminalSequences(conversation(() => appended, new HelperChat(() => undefined, "/tmp")).scroll.render(80).join("\n"));
  expect(text).toContain("OBSERVED_ERROR_VISIBLE"); expect(text).toContain("INSPECTOR_WARNING_VISIBLE");
});

test("data control sequences are sanitized before rendering, keeping only renderer-produced controls", () => {
  const evil = "\x1b]52;c;EVIL\x07\x1b[2J";
  const row = rowOf(helper({ prompt: evil + "Question", error: evil + "boom", messages: [{ id: "tool", kind: "tool", tool: "read", phase: "completed",
    text: evil + "line\n".repeat(500) }] }));
  const chat = new HelperChat(() => new TuiAltScreen(new RecordingTerminal()), "/tmp");
  const rendered = conversation(() => row, chat).scroll.render(80).join("\n");
  expect(rendered).not.toContain("\x1b]52"); expect(rendered).not.toContain("\x1b[2J");
  expect(stripTerminalSequences(rendered)).not.toContain("EVIL");
});

test("the back target covers the rendered ‹ Agents at every width, and a click on it goes back", () => {
  const row = { ...rowOf(helper({})), usage: { cost: 0.42, contextPercent: 37 }, pulse: [1, 4, 7, 3] };
  for (const width of [140, 80, 40, 24]) {
    const header = conversationHeader(row, width, NOW, agentColor);
    for (const line of header.lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const first = stripTerminalSequences(header.lines[0] ?? "");
    const at = first.indexOf("‹ Agents");
    expect(at).toBe(header.back.from);
    expect(header.back.to).toBe(at + "‹ Agents".length);
    expect(stripTerminalSequences(header.lines.join("\n"))).toContain("$0.42");
  }
  expect(conversationHeader(row, 140, NOW, agentColor).lines).toHaveLength(1);
  expect(conversationHeader(row, 40, NOW, agentColor).lines.length).toBeGreaterThan(1);

  let back = 0;
  const view = conversation(() => row, new HelperChat(() => undefined, "/tmp"), () => { back++; });
  const press = (x: number, y: number) => {
    const frame = renderLayoutFrame(view.scroll, 80, 20, () => {});
    for (const box of getLayoutBoxesAt(frame, x, y)) {
      const result = box.component.handleMouse?.({ type: "click", button: "left", x: x - box.rect.x, y: y - box.rect.y, screenX: x, screenY: y,
        width: box.rect.width, height: box.rect.height, shift: false, alt: false, ctrl: false, clickCount: 1 });
      if (result?.handled) return;
    }
  };
  const { back: range } = conversationHeader(row, 80, NOW, agentColor);
  press(range.to + 4, 0); expect(back).toBe(0);
  press(range.from, 0); expect(back).toBe(1);
  press(range.to - 1, 0); expect(back).toBe(2);
});

test("a child conversation shows the brief once, untimed steers after the replies, and its ▲ question line answers", () => {
  const child: ChildSnapshot = { id: "picker-ui", kind: "slice", call: "call-1", title: "Picker", brief: "BRIEF_ONCE_VISIBLE", owns: ["src/picker/"], after: [],
    profile: "default", model: "anthropic/model-a", state: "needs", createdAt: NOW - 9000, startedAt: NOW - 8000, edits: [],
    usage: { input: 1, output: 1, turns: 1 }, steers: ["STEER_ONE_VISIBLE", "STEER_TWO_VISIBLE"], pulse: [], history: "rn",
    question: { question: "QUESTION_VISIBLE", options: [{ label: "Grid" }, { label: "List" }], askedAt: NOW - 1000 },
    messages: [{ id: "a", kind: "assistant", text: "REPLY_VISIBLE", stopReason: "stop" }] };
  const row = childRow(child, { children: [child], mail: [], maxParallel: 3, startedAt: NOW - 9000 }, NOW);
  const answered: string[] = [];
  const view = new AgentConversation({ row: () => row, chat: new HelperChat(() => undefined, "/tmp"), color: agentColor, now: () => NOW,
    click: (event: TuiMouseEvent) => event.type === "click" && event.button === "left", focus() {}, back() {}, answer(key) { answered.push(key); } });
  const rows = view.scroll.render(80).map(line => stripTerminalSequences(line));
  const text = rows.join("\n");
  expect(text.split("BRIEF_ONCE_VISIBLE").length - 1).toBe(1);
  // childHTML: the header, a blank row, then the brief bubble with what the child owns inside it.
  expect(rows[0]).toMatch(/^ ‹ Agents picker-ui default · model-a ▲ needs you/);
  expect(rows[1]?.trim()).toBe("");
  expect(rows[2]?.startsWith("  brief from parent BRIEF_ONCE_VISIBLE")).toBeTrue();
  expect(rows[3]?.trim()).toBe("Owns: src/picker/ · no board access · no nested agents");
  expect(rows.some(line => line.startsWith("  you → picker-ui STEER_ONE_VISIBLE"))).toBeTrue();
  expect(text).toContain("▲ QUESTION_VISIBLE · answer in the dialog");
  const at = (needle: string) => rows.findIndex(line => line.includes(needle));
  expect(at("REPLY_VISIBLE")).toBeGreaterThan(at("BRIEF_ONCE_VISIBLE"));
  expect(at("STEER_ONE_VISIBLE")).toBeGreaterThan(at("REPLY_VISIBLE"));
  expect(at("STEER_TWO_VISIBLE")).toBeGreaterThan(at("STEER_ONE_VISIBLE"));
  const ask = at("QUESTION_VISIBLE");
  expect(ask).toBeGreaterThan(at("STEER_TWO_VISIBLE"));

  const press = (y: number) => {
    const frame = renderLayoutFrame(view.scroll, 80, rows.length + 4, () => {});
    for (const box of getLayoutBoxesAt(frame, 3, y)) {
      const result = box.component.handleMouse?.({ type: "click", button: "left", x: 3 - box.rect.x, y: y - box.rect.y, screenX: 3, screenY: y,
        width: box.rect.width, height: box.rect.height, shift: false, alt: false, ctrl: false, clickCount: 1 });
      if (result?.handled) return;
    }
  };
  press(at("STEER_ONE_VISIBLE"));
  expect(answered).toEqual([]);
  press(ask);
  expect(answered).toEqual([row.key]);
});

test("steers sit where they were sent, an untimed steer comes last, and the retention note sits under the brief", () => {
  const child: ChildSnapshot = { id: "picker-ui", kind: "slice", call: "call-1", title: "Picker", brief: "BRIEF_VISIBLE", owns: ["src/picker/"], after: [],
    profile: "default", model: "anthropic/model-a", state: "running", createdAt: NOW - 9000, startedAt: NOW - 8000, edits: [],
    usage: { input: 1, output: 1, turns: 1 }, steers: ["TIMED_STEER", "UNTIMED_STEER"], steeredAt: [NOW - 5000], pulse: [], history: "rr", dropped: 12,
    messages: [{ id: "a", kind: "assistant", text: "EARLY_REPLY", stopReason: "stop", at: NOW - 7000 },
      { id: "b", kind: "assistant", text: "LATE_REPLY", stopReason: "stop", at: NOW - 3000 }] };
  const row = childRow(child, { children: [child], mail: [], maxParallel: 3, startedAt: NOW - 9000 }, NOW);
  const rows = conversation(() => row, new HelperChat(() => undefined, "/tmp")).scroll.render(80).map(line => stripTerminalSequences(line));
  const at = (needle: string) => rows.findIndex(line => line.includes(needle));
  const note = rows.findIndex(line => line.trim().startsWith("! 12 earlier messages are not shown"));
  expect(note).toBe(at("Owns: src/picker/") + 2);
  expect(at("EARLY_REPLY")).toBeGreaterThan(note);
  expect(at("TIMED_STEER")).toBeGreaterThan(at("EARLY_REPLY"));
  expect(at("LATE_REPLY")).toBeGreaterThan(at("TIMED_STEER"));
  expect(at("UNTIMED_STEER")).toBeGreaterThan(at("LATE_REPLY"));
  expect(rows.filter(line => line.includes("not shown in this live preview"))).toHaveLength(1);
});

test("messages with other agents sit among the replies by their time, labelled sender to recipient; a helper without mail is unchanged", () => {
  const picker: ChildSnapshot = { id: "picker-ui", kind: "slice", call: "call-1", title: "Picker", brief: "BRIEF", owns: ["src/picker/"], after: [],
    profile: "default", model: "anthropic/model-a", state: "running", createdAt: NOW - 9000, startedAt: NOW - 8000, edits: [],
    usage: { input: 1, output: 1, turns: 1 }, steers: [], pulse: [], history: "rr",
    messages: [{ id: "a", kind: "assistant", text: "EARLY_REPLY", stopReason: "stop", at: NOW - 7000 },
      { id: "b", kind: "assistant", text: "LATE_REPLY", stopReason: "stop", at: NOW - 3000 }] };
  const slots: ChildSnapshot = { ...picker, id: "host-slots", messages: [] };
  const row = childRow(picker, { children: [slots, picker], maxParallel: 3, startedAt: NOW - 9000, mail: [
    { seq: 1, at: NOW - 5000, from: "host-slots", to: "picker-ui", text: "MAIL_IN", delivery: "delivered" },
    { seq: 2, at: NOW - 1000, from: "picker-ui", to: "parent", text: "MAIL_OUT", delivery: "undeliverable" },
  ] }, NOW);
  const rows = conversation(() => row, new HelperChat(() => undefined, "/tmp")).scroll.render(80).map(line => stripTerminalSequences(line));
  const at = (needle: string) => rows.findIndex(line => line.includes(needle));
  expect(at("MAIL_IN")).toBeGreaterThan(at("EARLY_REPLY"));
  expect(at("LATE_REPLY")).toBeGreaterThan(at("MAIL_IN"));
  expect(rows[at("MAIL_IN")]).toContain("host-slots → picker-ui");
  expect(rows[at("MAIL_OUT")]).toContain("picker-ui → parent");
  // An undelivered message says so inside its bubble, under its text.
  expect(rows.slice(at("MAIL_OUT") + 1).some(line => line.includes("!"))).toBeTrue();
  const plain = rowOf(helper({}));
  const before = conversation(() => plain, new HelperChat(() => undefined, "/tmp")).scroll.render(80);
  expect(conversation(() => ({ ...plain, mail: [] }), new HelperChat(() => undefined, "/tmp")).scroll.render(80)).toEqual(before);
});

test("a running agent's page follows its end until the owner scrolls up; a finished one opens at the top", () => {
  const replies = (count: number) => Array.from({ length: count }, (_, index) => ({ id: String(index), kind: "assistant" as const,
    text: `Reply MARK${index}X.\n\n` + "More words here.\n\n".repeat(3), stopReason: "stop" as const }));
  let row = rowOf(helper({ messages: replies(6) }));
  const view = conversation(() => row, new HelperChat(() => undefined, "/tmp"));
  const shown = () => stripTerminalSequences(renderLayoutFrame(view.scroll, 80, 10, () => {}).lines.join("\n"));
  expect(shown()).toContain("MARK5X");
  expect(view.following).toBeTrue();
  row = rowOf(helper({ messages: replies(7) }));
  expect(shown()).toContain("MARK6X");
  expect(view.following).toBeTrue();

  view.scroll.scrollBy(-8);
  const top = view.scroll.scrollTop;
  row = rowOf(helper({ messages: replies(8) }));
  expect(shown()).not.toContain("MARK7X");
  expect(view.scroll.scrollTop).toBe(top);
  expect(view.following).toBeFalse();
  view.scroll.scrollBy(1000);
  expect(view.following).toBeTrue();
  expect(shown()).toContain("MARK7X");

  row = rowOf(helper({ phase: "completed", endedAt: NOW - 1000, messages: replies(8) }));
  view.restore(row.key);
  expect(shown()).toContain("Question");
  expect(view.scroll.scrollTop).toBe(0);
});

/** A recording of `count` turns after its prompt, one turn a page; `stamp` names its snapshot, and a cursor of another snapshot is stale. */
function recording(count: () => number, stamp: () => string) {
  const reads: string[] = [];
  const page = (index: number): TranscriptPage => ({ stamp: stamp(), rows: index === 0
    ? [{ id: "r0", kind: "user", text: "RECORDED_PROMPT", record: 0, item: 0 }]
    : [{ id: `t${index}`, kind: "thinking", text: `THOUGHT_${index}`, record: index * 100, item: 0 },
      { id: `a${index}`, kind: "assistant", text: `RECORDED_${index}_REPLY\n\n` + "More words here.\n\n".repeat(4), record: index * 100, item: 1 },
      { id: `c${index}`, kind: "call", tool: "read", callId: `call-${index}`, text: `{"path":"src/file-${index}.ts"}`, record: index * 100, item: 2 },
      { id: `o${index}`, kind: "tool", tool: "read", callId: `call-${index}`, text: "one\ntwo\n", record: index * 100 + 50, item: 0 }],
    ...(index ? { older: `${stamp()}:${index - 1}` } : {}), ...(index < count() ? { newer: `${stamp()}:${index + 1}` } : {}) });
  const read: TranscriptRead = async (_id, request) => {
    reads.push(request.cursor ?? "latest");
    if (request.cursor === undefined) return page(count());
    const [of, index] = request.cursor.split(":");
    if (of !== stamp()) throw new Error("Transcript changed; load latest messages");
    return page(Number(index));
  };
  return { read, reads };
}
function settle(): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}
/** The view's rows without its scrollbar column, which moves whenever the content grows. */
function reading(view: AgentConversation, rows = 12): string {
  return renderLayoutFrame(view.scroll, 80, rows, () => {}).lines.map(line => stripTerminalSequences(line).slice(0, 78).trimEnd()).join("\n");
}
/** A helper's row with a saved session. */
function recorded(entry: Partial<HelperInspection> = { phase: "completed" }): AgentRow {
  const live = rowOf(helper(entry));
  return { ...live, capabilities: { ...live.capabilities, transcript: true } };
}
/**
 * The view over `read`, drawn whenever it asks for a frame, as the TUI draws it. `rest` returns once nothing is being
 * read and no frame is asked for; `open` opens the agent and rests.
 */
function reader(row: () => AgentRow, read: TranscriptRead, more: Partial<AgentConversationHost> = {}) {
  let asked = false;
  const view = new AgentConversation({ row, chat: new HelperChat(() => undefined, "/tmp"), color: agentColor, now: () => NOW,
    click: () => false, focus() {}, back() {}, read, changed: () => { asked = true; }, ...more });
  const rest = async (rows = 12) => { do { asked = false; reading(view, rows); await settle(); } while (asked); };
  const open = async (rows = 12) => { view.restore(row().key); view.refresh(); await settle(); await rest(rows); };
  return { view, rest, open };
}
const whole = (view: AgentConversation) => stripTerminalSequences(view.scroll.render(80).join("\n"));
const times = (text: string, needle: string) => text.split(needle).length - 1;

test("a recording is drawn as the live conversation and read by scrolling: no page that joins moves the reader", async () => {
  const row = recorded({ phase: "completed", truncated: true, dropped: 200 });
  const { read, reads } = recording(() => 9, () => "same");
  const { view, open } = reader(() => row, read);
  await open();
  // It opens at its latest turn: the reply as words, the call on one row with its result.
  const opened = reading(view);
  expect(opened).toContain("More words here.");
  expect(opened).toContain("✓ read src/file-9.ts · 2 lines");
  // Pages beyond the view were read ahead of the reader, who is still at the end.
  expect(reads.length).toBeGreaterThan(1);
  expect(reads).not.toContain("same:0");
  expect(view.following).toBeTrue();
  // Scrolling up reads each earlier page once, before the reader reaches it, and the rows on screen stay put.
  for (let step = 0; step < 60 && view.scroll.scrollTop > 0; step++) {
    view.scroll.scrollBy(-5);
    const before = reading(view); await settle();
    expect(reading(view)).toBe(before);
  }
  expect(reads).toEqual(["latest", ...Array.from({ length: 9 }, (_, index) => `same:${8 - index}`)]);
  expect(reading(view)).toContain("prompt from parent RECORDED_PROMPT");
  const text = whole(view);
  for (let index = 1; index <= 9; index++) expect(times(text, `RECORDED_${index}_REPLY`)).toBe(1);
  expect(text).toContain("THOUGHT_3");
  // The recording stands for the live projection, and nothing asks to be clicked.
  expect(text).not.toContain("Visible response");
  expect(text).not.toMatch(/Show |Load /);
});

test("a recording that grew is read again around the reader's row, and reopening returns to that row", async () => {
  const row = recorded();
  let count = 9, stamp = "one";
  const { read, reads } = recording(() => count, () => stamp);
  const { view, open, rest } = reader(() => row, read);
  await open();
  view.scroll.scrollBy(-30); await rest();
  const resident = reads.length;
  expect(reads).not.toContain("one:2");
  // The agent wrote another turn: the cursors of the pages on screen no longer read the file.
  count = 10; stamp = "two";
  view.scroll.scrollBy(-20);
  const before = reading(view); await rest();
  expect(reads[resident]?.startsWith("one:")).toBeTrue();
  expect(reads[resident + 1]).toBe("latest");
  expect(reading(view)).toBe(before);
  expect(whole(view)).toContain("RECORDED_10_REPLY");
  // Leaving and coming back after it grew again: the same row, read back from the new end.
  view.remember();
  count = 11; stamp = "three";
  await open();
  expect(reading(view)).toBe(before);
  // It grew once more, and they ask for its end once it is read again and before a frame shows it: its new end.
  count = 12; stamp = "four";
  expect(view.navigate("older")).toBeTrue(); await settle();
  view.scroll.scrollToEnd(); await rest();
  expect(reading(view)).toContain("✓ read src/file-12.ts · 2 lines");
});

test("a running agent's unrecorded work follows its recording once, and the settled recording replaces it", async () => {
  const call = toolMessageId("c1");
  let row = recorded({ messages: [
    { id: "assistant-1", kind: "assistant", text: "FIRST_REPLY", stopReason: "toolUse", hasToolCalls: true },
    { id: call, kind: "tool", tool: "read", phase: "running", arguments: '{"path":"a.ts"}', text: "" },
    { id: "assistant-2", kind: "assistant", text: "TYPING_NOW" }] });
  let rows: TranscriptRow[] = [
    { id: "u", kind: "user", text: "Question", record: 0, item: 0 },
    { id: "a", kind: "assistant", text: "FIRST_REPLY", record: 10, item: 0 },
    { id: "c", kind: "call", tool: "read", callId: call, text: '{"path":"a.ts"}', record: 10, item: 1 }];
  let unreadable = false;
  const { view } = reader(() => row, async () => {
    if (unreadable) { unreadable = false; throw new Error("transient read failure"); }
    return { stamp: String(rows.length), rows };
  });
  view.restore(row.key);
  view.refresh(); await settle();
  const live = whole(view);
  expect(times(live, "FIRST_REPLY")).toBe(1);
  expect(times(live, "read a.ts")).toBe(1);
  expect(live).not.toContain("✓ read a.ts");
  expect(times(live, "TYPING_NOW")).toBe(1);
  // The reply ends before the recording is read again: it stays on screen.
  row = { ...row, conversation: { ...row.conversation!, messages: row.conversation!.messages.map(message =>
    message.id === "assistant-2" ? { ...message, stopReason: "stop" as const } : message) } };
  expect(times(whole(view), "TYPING_NOW")).toBe(1);
  rows = [...rows, { id: "o", kind: "tool", tool: "read", callId: call, text: "one\n", record: 20, item: 0 },
    { id: "f", kind: "assistant", text: "FINAL_ANSWER", record: 30, item: 0 }];
  row = { ...row, state: "done" };
  // The read that follows its settlement fails once: the next tick still owes it.
  unreadable = true;
  view.refresh(); await settle();
  expect(whole(view)).not.toContain("FINAL_ANSWER");
  view.refresh(); await settle();
  const final = whole(view);
  expect(times(final, "FINAL_ANSWER")).toBe(1);
  expect(final).toContain("✓ read a.ts");
  expect(final).not.toContain("TYPING_NOW");
});

test("a reply that ends while the recording is read is drawn once: only a read that none ended during is shown", async () => {
  let row = recorded({ messages: [{ id: "assistant-1", kind: "assistant", text: "REPLY_1" }] });
  let rows: TranscriptRow[] = [{ id: "u", kind: "user", text: "RECORDED_PROMPT", record: 0, item: 0 }];
  // During each of the next `ending` reads the reply being written ends and is in the file read, and the next begins.
  let ending = 3, reads = 0;
  const { view } = reader(() => row, async () => {
    reads++;
    await Promise.resolve();
    if (ending > 0) {
      ending--;
      const messages = row.conversation!.messages, last = messages.length;
      rows = [...rows, { id: `a${last}`, kind: "assistant", text: `REPLY_${last}`, record: last * 10, item: 0 }];
      row = { ...row, conversation: { ...row.conversation!, messages: [...messages.map(message => ({ ...message, stopReason: "stop" as const })),
        { id: `assistant-${last + 1}`, kind: "assistant", text: `REPLY_${last + 1}` }] } };
    }
    return { stamp: String(rows.length), rows };
  });
  view.restore(row.key);
  view.refresh(); await settle();
  // No read found the recording at rest: none is shown, and what the session holds stays as it was.
  expect(reads).toBe(3);
  let text = whole(view);
  expect(text).not.toContain("RECORDED_PROMPT");
  for (let reply = 1; reply <= 4; reply++) expect(times(text, `REPLY_${reply}`)).toBe(1);
  // A reply ends during the next tick's first read and none during its second, which is shown.
  ending = 1;
  view.refresh(); await settle();
  expect(reads).toBe(5);
  text = whole(view);
  expect(text).toContain("RECORDED_PROMPT");
  for (let reply = 1; reply <= 5; reply++) expect(times(text, `REPLY_${reply}`)).toBe(1);
});

test("pages that draw no rows are not traded across a full window, and a push against its edge reads on", async () => {
  const row = recorded();
  // 80 pages of one tool result too long for a page each: only the oldest page has a row to draw.
  const reads: string[] = [];
  const { view, open, rest } = reader(() => row, async (_id, request) => {
    const index = request.cursor === undefined ? 79 : Number(request.cursor);
    reads.push(String(index));
    return { stamp: "same", rows: index ? [] : [{ id: "u", kind: "user", text: "RECORDED_PROMPT", record: 0, item: 0 }],
      ...(index ? { older: String(index - 1) } : {}), ...(index < 79 ? { newer: String(index + 1) } : {}) };
  });
  await open();
  // Reading ahead fills the window and stops: the newest page, which the reader is on, stays resident.
  expect(reads).toEqual(Array.from({ length: 64 }, (_, index) => String(79 - index)));
  // The owner pushes up against the first row: the read goes on to the recording's start, and rests there.
  view.scroll.scrollBy(-3); await settle(); await rest();
  expect(reads).toHaveLength(80);
  expect(reading(view)).toContain("RECORDED_PROMPT");
  await rest();
  expect(reads).toHaveLength(80);
});

test("the bottom of the resident pages is not the recording's end: a page joins under the reader's rows, and leaving keeps that row", async () => {
  const row = recorded();
  const { read, reads } = recording(() => 70, () => "same");
  const { view, open, rest } = reader(() => row, read);
  await open();
  // Up to the first row: the newest pages leave the 64-page window.
  for (let step = 0; step < 400 && !(view.scroll.scrollTop === 0 && reads.includes("same:0")); step++) { view.scroll.scrollBy(-200); await rest(); }
  const before = reads.length;
  // Straight down to the last resident row, where the view would follow an end.
  view.scroll.scrollBy(100000);
  const bottom = reading(view);
  view.remember();
  await settle();
  expect(reads.length).toBe(before + 1);
  expect(reading(view)).toBe(bottom);
  // The place kept a moment ago was that row, not the recording's end.
  await open();
  expect(reading(view)).toBe(bottom);
});

test("a jump to the end while the newest pages are not resident reads the recording's end, unless the owner scrolls away before it is shown", async () => {
  const row = recorded();
  const { read, reads } = recording(() => 70, () => "same");
  let held: PromiseWithResolvers<void> | undefined, unreadable = false;
  const { view, open, rest } = reader(() => row, async (id, request) => {
    await held?.promise;
    if (unreadable) throw new Error("transient read failure");
    return read(id, request);
  });
  await open();
  for (let step = 0; step < 400 && !(view.scroll.scrollTop === 0 && reads.includes("same:0")); step++) { view.scroll.scrollBy(-200); await rest(); }
  // Pi's jump control asks the scroll view for its end. The owner scrolls up while that end is read: it moves nothing.
  held = Promise.withResolvers();
  view.scroll.scrollToEnd(); await settle();
  view.scroll.scrollBy(-5);
  const place = reading(view);
  held.resolve(); held = undefined; await rest();
  expect(reading(view)).toBe(place);
  // They scroll up once it is read, before a frame has shown it: it moves nothing either.
  const before = reads.length;
  view.scroll.scrollToEnd(); await settle();
  expect(reads.length).toBeGreaterThan(before);
  view.scroll.scrollBy(-5);
  expect(reading(view)).toBe(place);
  await rest();
  expect(reading(view)).toBe(place);
  // Nor is a read of that end owed when it fails after they scrolled away.
  held = Promise.withResolvers(); unreadable = true;
  view.scroll.scrollToEnd(); await settle();
  view.scroll.scrollBy(-5);
  held.resolve(); held = undefined; await rest(); unreadable = false;
  expect(whole(view)).not.toContain("could not be read");
  // Left alone, the jump lands on the recording's end.
  view.scroll.scrollToEnd(); await rest();
  expect(reading(view)).toContain("✓ read src/file-70.ts · 2 lines");
  expect(view.following).toBeTrue();
});

test("the end asked for while a full window changes is the recording's end: a page joining at either side, a read around the reader's row", async () => {
  const row = recorded();
  let count = 70, stamp = "one";
  const { read, reads } = recording(() => count, () => stamp);
  const { view, open, rest } = reader(() => row, read);
  await open();
  // Up until the window is full and has given up the newest turn alone.
  for (let step = 0; step < 400 && !reads.includes("one:6"); step++) { view.scroll.scrollBy(-10); await rest(); }
  expect(reads).not.toContain("one:5");
  // Down to the last resident row, where that turn is read. They ask for the end before a frame joins it.
  view.scroll.scrollBy(100000); await settle();
  expect(reads.at(-1)).toBe("one:70");
  view.scroll.scrollToEnd(); await rest();
  expect(reading(view)).toContain("✓ read src/file-70.ts · 2 lines");
  expect(view.following).toBeTrue();
  // Up a little, where one more earlier page is read. They ask for the end before a frame joins it: the window
  // keeps its end, so the newest turn is not read again.
  view.scroll.scrollBy(-30); await rest();
  expect(view.navigate("older")).toBeTrue(); await settle();
  expect(reads.at(-1)).toBe("one:6");
  view.scroll.scrollToEnd(); await rest();
  expect(reading(view)).toContain("✓ read src/file-70.ts · 2 lines");
  expect(view.following).toBeTrue();
  expect(reads.filter(cursor => cursor === "one:70")).toHaveLength(1);
  // The agent wrote another turn while they read above. The read back to their row is of more pages than stay
  // resident: it gives up the newest ones.
  view.scroll.scrollBy(-30); await rest();
  count = 71; stamp = "two";
  expect(view.navigate("older")).toBeTrue(); await settle();
  expect(reads.filter(cursor => cursor.startsWith("two:")).length).toBeGreaterThanOrEqual(64);
  // They ask for the end before a frame shows what was read.
  view.scroll.scrollToEnd(); await rest();
  expect(reading(view)).toContain("✓ read src/file-71.ts · 2 lines");
  expect(view.following).toBeTrue();
});

test("the end asked for while a recording is reopened at a kept row is where it opens", async () => {
  const row = recorded();
  const { read } = recording(() => 9, () => "same");
  let held: PromiseWithResolvers<void> | undefined;
  const { view, open, rest } = reader(() => row, async (id, request) => { await held?.promise; return read(id, request); });
  await open();
  view.scroll.scrollBy(-30); await rest();
  view.remember();
  // Reopened: its first read is held, and what the session holds of the agent is on screen.
  held = Promise.withResolvers();
  view.restore(row.key); view.refresh(); reading(view);
  view.scroll.scrollToEnd();
  held.resolve(); held = undefined; await rest();
  expect(reading(view)).toContain("✓ read src/file-9.ts · 2 lines");
  expect(view.following).toBeTrue();
});

test("reopening a reply that was read from its last rows returns to the same rows once its first rows are read", async () => {
  const row = recorded();
  const lines = (from: number, to: number) => Array.from({ length: to - from }, (_, index) => `REPLY_ROW_${from + index}`).join("\n\n");
  const first = lines(0, 20) + "\n\n";
  const pages: TranscriptPage[] = [
    { stamp: "same", rows: [{ id: "u", kind: "user", text: "RECORDED_PROMPT", record: 0, item: 0 }], newer: "1" },
    { stamp: "same", rows: [{ id: "a0", kind: "assistant", text: first, record: 100, item: 0, more: true }], older: "0", newer: "2" },
    { stamp: "same", rows: [{ id: "a1", kind: "assistant", text: lines(20, 40), record: 100, item: 0, from: first.length }], older: "1" }];
  let reach = false;
  const { view } = reader(() => row, async (_id, request) => pages[request.cursor === undefined ? 2 : Number(request.cursor)]!, { canInteract: () => reach });
  // Only the newest page is resident: the reader is in the reply's last rows.
  view.restore(row.key);
  view.refresh(); await settle(); reading(view, 8);
  view.scroll.scrollBy(-9);
  const place = reading(view, 8);
  expect(place).toContain("REPLY_ROW_3");
  expect(whole(view)).not.toContain("REPLY_ROW_19");
  view.remember();
  reach = true;
  view.restore(row.key);
  view.refresh(); await settle(); await settle();
  expect(reading(view, 8)).toBe(place);
  expect(whole(view)).toContain("REPLY_ROW_0");
});

test("a reply longer than the resident window keeps the reader's rows as the window slides along it", async () => {
  const row = recorded();
  // One reply over 80 pages, three paragraphs a page, after the prompt's page.
  const slice = (index: number) => [0, 1, 2].map(line => `SLICE_${index}_ROW_${line}\n\n`).join("");
  const size = slice(1).length;
  const { view, open } = reader(() => row, async (_id, request) => {
    const index = request.cursor === undefined ? 80 : Number(request.cursor);
    return { stamp: "same", rows: index === 0 ? [{ id: "u", kind: "user", text: "RECORDED_PROMPT", record: 0, item: 0 }]
      : [{ id: `a${index}`, kind: "assistant", text: slice(index), record: 100, item: 0, ...(index > 1 ? { from: (index - 1) * size } : {}), ...(index < 80 ? { more: true } : {}) }],
      ...(index ? { older: String(index - 1) } : {}), ...(index < 80 ? { newer: String(index + 1) } : {}) };
  });
  await open();
  for (let step = 0; step < 400 && !whole(view).includes("SLICE_1_ROW_0"); step++) {
    view.scroll.scrollBy(-4);
    const before = reading(view); await settle();
    // An earlier slice joins at the reply's first rows; then the window gives up a slice at its last rows.
    expect(reading(view)).toBe(before);
    expect(reading(view)).toBe(before);
  }
  expect(whole(view)).not.toContain("SLICE_80_ROW_0");
});
