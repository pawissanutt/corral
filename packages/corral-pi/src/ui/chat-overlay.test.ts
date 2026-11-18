import { expect, test } from "bun:test";
import { Container, ScrollView, stripTerminalSequences, Text } from "@earendil-works/pi-tui";
import { emptyChatLines, jumpLabel, TranscriptWatch } from "./chat-overlay.ts";

const rows = (count: number) => Array.from({ length: count }, (_, index) => `ROW_${index}`).join("\n");
const noop = () => {};

test("new rows count only while the reader is not following, and End's jump resumes following", () => {
  const document = new Text(rows(50), 0, 0), transcript = new ScrollView(document, { follow: "end" });
  const watch = new TranscriptWatch(transcript);
  transcript.updateLayout(50, 10, noop);
  expect(transcript.isFollowingEnd).toBeTrue(); expect(watch.newRows).toBe(0);
  // Following, growth is not "new below": the reader sees it.
  transcript.updateLayout(55, 10, noop); expect(watch.newRows).toBe(0);
  transcript.scrollBy(-20); expect(transcript.isFollowingEnd).toBeFalse();
  transcript.updateLayout(62, 10, noop); expect(watch.newRows).toBe(7);
  expect(jumpLabel(watch.newRows, "End")).toContain("7 new rows");
  watch.jump(); expect(transcript.isFollowingEnd).toBeTrue(); expect(watch.newRows).toBe(0);
  expect(jumpLabel(0, undefined)).toContain("Jump to latest message");
});

test("the unread mark sets once an entry is added behind another view, clears when Chat opens, and never renders hidden history", () => {
  // Pi's shape: a document holding the chat container, which holds one component per entry.
  const chat = new Container(), document = new Container();
  chat.addChild(new Text(rows(3), 0, 0)); document.addChild(chat);
  let renders = 0;
  const render = document.render.bind(document);
  document.render = width => { renders++; return render(width); };
  const watch = new TranscriptWatch(new ScrollView(document, { follow: "end" }));
  expect(watch.unread()).toBeFalse(); // The first frame of the other view measures from here.
  expect(watch.unread()).toBeFalse(); expect(watch.unread()).toBeFalse();
  expect(renders).toBe(0);
  chat.addChild(new Text("NEW_ENTRY", 0, 0));
  expect(watch.unread()).toBeTrue();
  watch.back();
  expect(watch.unread()).toBeFalse(); expect(watch.unread()).toBeFalse();
  expect(renders).toBe(0);
});

test("the empty state shows until the first transcript row and never comes back", () => {
  const document = new Text("", 0, 0), watch = new TranscriptWatch(new ScrollView(document, { follow: "end" }));
  expect(watch.empty(80)).toBeTrue();
  document.setText("first row");
  expect(watch.empty(80)).toBeFalse();
  document.setText("");
  expect(watch.empty(80)).toBeFalse();
});

test("the empty Chat names who receives the message per role, with send, F1 and quit", () => {
  const text = (role: "build" | "assistant" | "worker") =>
    emptyChatLines({ identity: { store: "s", project: "demo", role, ...(role === "assistant" ? { assistant: "A-7" } : {}) }, send: "⏎", quit: "Ctrl+C" }, 90)
      .map(line => stripTerminalSequences(line)).join("\n");
  expect(text("build")).toContain("Build session, unbound."); expect(text("build")).toContain("/finish");
  expect(text("worker")).toContain("Waiting for corral's first instruction"); expect(text("worker")).not.toContain("/finish");
  expect(text("assistant")).toContain("A-7");
  for (const role of ["build", "assistant", "worker"] as const) {
    expect(text(role)).toContain("⏎ send"); expect(text(role)).toContain("F1 keys"); expect(text(role)).toContain("Ctrl+C twice quits");
  }
});
