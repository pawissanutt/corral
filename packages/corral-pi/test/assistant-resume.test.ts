// `/resume` in a supervised assistant: the Corral picker, its refusals and the switch request.
import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../src/bridge/events.ts";
import {
  AssistantResume, type Chooser, type ResumeRow, SWITCH_NOTICE_FILE, SWITCH_REQUEST_FILE, choose, filterRows, resumeRows, rowLabel,
  rowLines,
} from "../src/host/assistant-resume.ts";
import { visibleWidth } from "@earendil-works/pi-tui";

const LIST = [
  { session: "A-1", ended: null, live: true, focus: "workspace", summary: "current work", last_activity: "2026-11-04T10:00:00Z" },
  { session: "A-2", ended: null, live: false, focus: "workspace", summary: "storage spike", last_activity: "2026-11-04T09:00:00Z" },
  { session: "A-3", ended: "2026-11-03T00:00:00Z", live: false, focus: "T-12", summary: "", last_activity: "2026-11-03T00:00:00Z" },
  { session: "A-4", ended: null, live: true, focus: "workspace", summary: "elsewhere", last_activity: "2026-11-04T08:00:00Z" },
];

function harness(lists: unknown[], pick: (rows: readonly ResumeRow[]) => ResumeRow | undefined) {
  const dir = mkdtempSync(join(tmpdir(), "corral-resume-"));
  const events = new EventLog(1);
  const notices: string[] = [];
  let requests = 0, reads = 0, shown: readonly ResumeRow[] = [];
  const ui = { notify: (message: string) => { notices.push(message); } } as unknown as ExtensionUIContext;
  const chooser: Chooser = async (_ui, rows) => { shown = rows; return pick(rows); };
  const resume = new AssistantResume({
    assistant: "A-1", generation: 3, runtimeDir: dir, events, idle: () => true, choose: chooser,
    read: async () => { const value = lists[Math.min(reads++, lists.length - 1)]; if (value instanceof Error) throw value; return value; },
    request: () => { requests++; },
  });
  const kinds = () => events.since(0, 1_000_000, 1000).events.map(event => event.kind);
  return { dir, resume, ui, notices, kinds, requests: () => requests, shown: () => shown,
    request: () => existsSync(join(dir, SWITCH_REQUEST_FILE)) ? JSON.parse(readFileSync(join(dir, SWITCH_REQUEST_FILE), "utf8")) : undefined,
    done: () => rmSync(dir, { recursive: true, force: true }) };
}

test("rows carry A-ID, summary, last activity and availability, newest first, and search all of them", () => {
  const rows = resumeRows(LIST, "A-1");
  expect(rows.map(row => [row.id, row.availability])).toEqual([["A-1", "current"], ["A-2", "stopped"], ["A-4", "live"], ["A-3", "ended"]]);
  expect(rowLabel(rows[1]!)).toBe("A-2 · storage spike · 2026-11-04 09:00 · stopped · resumable");
  expect(rowLabel(rows[3]!)).toContain("focus T-12");
  expect(filterRows(rows, "spike").map(row => row.id)).toEqual(["A-2"]);
  expect(filterRows(rows, "ended").map(row => row.id)).toEqual(["A-3"]);
  expect(() => resumeRows({}, "A-1")).toThrow("not a list");
});

test("selecting a stopped session revalidates, writes this generation's request and asks the supervisor", async () => {
  const h = harness([LIST], rows => rows.find(row => row.id === "A-2"));
  try {
    await h.resume.open(h.ui, () => true);
    expect(h.shown().length).toBe(4);
    expect(h.request()).toEqual({ version: 1, assistant: "A-1", generation: 3, target: "A-2" });
    expect(h.requests()).toBe(1);
    expect(h.kinds()).toContain("assistant_resume_requested");
  } finally { h.done(); }
});

test("cancel, empty and unavailable lists leave the conversation and binding untouched", async () => {
  for (const [lists, expected] of [
    [[LIST], undefined],
    [[[LIST[0]]], "no other assistant sessions"],
    [[new Error("list failed")], "assistant sessions unavailable: list failed"],
  ] as const) {
    const h = harness([...lists], () => undefined);
    try {
      await h.resume.open(h.ui, () => true);
      expect([h.requests(), h.request()]).toEqual([0, undefined]);
      if (expected) expect(h.notices.join("\n")).toContain(expected);
      else expect(h.kinds()).toContain("assistant_resume_cancelled");
    } finally { h.done(); }
  }
});

test("current, live, ended and since-vanished or since-started targets are refused visibly", async () => {
  const started = LIST.map(row => row.session === "A-2" ? { ...row, live: true } : row);
  for (const [target, lists, expected] of [
    ["A-1", [LIST], "is the current session"],
    ["A-4", [LIST], "running in another pane"],
    ["A-3", [LIST], "stays ended"],
    ["A-2", [LIST, LIST.filter(row => row.session !== "A-2")], "no longer an assistant session"],
    ["A-2", [LIST, started], "running in another pane"],
  ] as const) {
    const h = harness([...lists], rows => rows.find(row => row.id === target));
    try {
      await h.resume.open(h.ui, () => true);
      expect([target, h.requests(), h.request()]).toEqual([target, 0, undefined]);
      expect(h.notices.join("\n")).toContain(expected);
      expect(h.kinds()).toContain("assistant_resume_refused");
    } finally { h.done(); }
  }
});

test("a busy host refuses before reading, and the supervisor's notice is shown once at startup", async () => {
  const h = harness([LIST], rows => rows[1]);
  try {
    await h.resume.open(h.ui, () => false);
    expect(h.notices).toEqual(["corral: wait for the current work to finish before /resume"]);
    expect(h.requests()).toBe(0);
    writeFileSync(join(h.dir, SWITCH_NOTICE_FILE), "corral: /resume did not switch: x. Continuing A-1.");
    let start: ((event: { reason: string }, ctx: unknown) => void) | undefined;
    const pi = { registerCommand: () => undefined, on: (_: string, handler: typeof start) => { start = handler; } };
    h.resume.extend(pi as never);
    const ctx = { hasUI: true, ui: h.ui };
    start?.({ reason: "startup" }, ctx);
    start?.({ reason: "startup" }, ctx);
    expect(h.notices.filter(text => text.includes("did not switch"))).toHaveLength(1);
  } finally { h.done(); }
});

test("the picker renders, searches and cancels from keyboard input", async () => {
  const rows = resumeRows(LIST, "A-1");
  const frames: string[][] = [];
  const ui = {
    custom: (factory: Parameters<ExtensionUIContext["custom"]>[0]) => new Promise(resolve => {
      const theme = { bold: (s: string) => s, fg: (_: string, s: string) => s };
      const component = factory({ terminal: { rows: 24 }, requestRender: () => undefined } as never, theme as never, {} as never,
        resolve as never) as { render(width: number): string[]; handleInput(data: string): void };
      frames.push(component.render(100));
      for (const key of "spike") component.handleInput(key);
      frames.push(component.render(100));
      component.handleInput("\r");
    }),
  } as unknown as ExtensionUIContext;
  expect((await choose(ui, rows))?.id).toBe("A-2");
  // Long and wide summaries are cut in terminal cells; activity and availability always remain.
  for (const summary of ["x".repeat(120), "会".repeat(40)]) {
    const row = resumeRows([{ ...LIST[1], summary }], "A-1")[0]!;
    for (const width of [80, 30]) {
      const lines = rowLines(row, width, "› ");
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      expect(lines.join(" ")).toContain("2026-11-04 09:00");
      expect(lines.join(" ")).toContain("stopped · resumable");
    }
  }
  expect(frames[0]!.join("\n")).toContain("A-3 · focus T-12");
  expect(frames[1]!.filter(line => line.includes("A-"))).toEqual(["› A-2 · storage spike · 2026-11-04 09:00 · stopped · resumable"]);
  writeFileSync(join(tmpdir(), "corral-resume-frames.txt"), frames.map(frame => frame.join("\n")).join("\n---\n"));
});
