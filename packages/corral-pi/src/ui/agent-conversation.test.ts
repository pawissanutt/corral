import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { TuiAltScreen, stripTerminalSequences, visibleWidth, type TuiMouseEvent } from "@earendil-works/pi-tui";
import { getLayoutBoxesAt, renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { AgentConversation, conversationHeader } from "./agent-conversation.ts";
import { helperRows, type AgentRow } from "./agents-model.ts";
import { agentColor } from "./agent-hues.ts";
import { HelperChat } from "./helper-chat.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";
import type { ChildSnapshot } from "../build/types.ts";
import { childRow } from "./build/fleet-rows.ts";

beforeAll(() => initTheme("dark", false));
const NOW = 1_000_000;
const base: PanelSnapshot = { bound: true, model: "m", effort: "off", streaming: false, context: undefined, tokens: 0, tools: [], contextFiles: [], skills: [],
  ledger: null, ledgerStale: false, steering: 0, followUps: 0, auxiliary: null, jobs: [] };
function helper(extra: Partial<HelperInspection>): HelperInspection {
  return { request: "r", kind: "explore", model: "provider/model-a", generation: 1, phase: "running", createdAt: NOW - 9000, startedAt: NOW - 8000,
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
  const note = rows.findIndex(line => line.trim().startsWith("! 12 earlier messages were dropped"));
  expect(note).toBe(at("Owns: src/picker/") + 2);
  expect(at("EARLY_REPLY")).toBeGreaterThan(note);
  expect(at("TIMED_STEER")).toBeGreaterThan(at("EARLY_REPLY"));
  expect(at("LATE_REPLY")).toBeGreaterThan(at("TIMED_STEER"));
  expect(at("UNTIMED_STEER")).toBeGreaterThan(at("LATE_REPLY"));
  expect(rows.filter(line => line.includes("dropped by the retention limit"))).toHaveLength(1);
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
