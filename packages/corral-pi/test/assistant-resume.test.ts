// `/resume` in a supervised assistant: the Corral picker, its refusals and the switch request.
import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";

import { EventLog } from "../src/bridge/events.ts";
import { AssistantResume, type Chooser, type ResumeRow, SWITCH_NOTICE_FILE, SWITCH_REQUEST_FILE, resumeRows } from "../src/host/assistant-resume.ts";
import { chooseResume, resumePopupRows } from "../src/ui/resume-list.ts";

interface Surface {
  focused: boolean;
  render(width: number): string[];
  handleInput(data: string): void;
  handleMouse?(event: unknown): unknown;
}

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

test("rows carry A-ID, summary, last activity and availability, newest first", () => {
  const rows = resumeRows(LIST, "A-1");
  expect(rows.map(row => [row.id, row.availability])).toEqual([["A-1", "current"], ["A-2", "stopped"], ["A-4", "live"], ["A-3", "ended"]]);
  expect(rows[1]).toEqual({ id: "A-2", title: "storage spike", lastActivity: "2026-11-04T09:00:00Z", availability: "stopped" });
  expect(rows[3]!.title).toBe("focus T-12");
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

/** Runs `chooseResume` against a stand-in `ui.custom`; `steps` must end with a pick or Esc. */
function drive(rows: readonly ResumeRow[], steps: (surface: Surface) => void, columns = 100): Promise<ResumeRow | undefined> {
  const ui = {
    custom: (factory: Parameters<ExtensionUIContext["custom"]>[0]) => new Promise(resolve => {
      const theme = { bold: (s: string) => s, fg: (_: string, s: string) => s };
      const surface = factory({ terminal: { rows: 24, columns }, requestRender: () => undefined } as never, theme as never, {} as never,
        resolve as never) as unknown as Surface;
      surface.focused = true;
      steps(surface);
    }),
  } as unknown as ExtensionUIContext;
  return chooseResume(ui, rows);
}
const type = (surface: Surface, text: string) => { for (const key of text) surface.handleInput(key); };
const shown = (surface: Surface, width: number) => surface.render(width).map(line => stripTerminalSequences(line));
const click = (y: number) => ({ type: "click", button: "left", x: 6, y, shift: false, alt: false, ctrl: false });

test("searching by words, date or state narrows the list, and a stopped row is returned exactly", async () => {
  const rows = resumeRows(LIST, "A-1");
  expect(await drive(rows, surface => { type(surface, "spike"); surface.handleInput("\r"); })).toBe(rows[1]!);
  for (const [query, id, other] of [["2026-11-03", "A-3", "A-2"], ["09:00", "A-2", "A-3"], ["another pane", "A-4", "A-2"]] as const) {
    let frame = "";
    await drive(rows, surface => { type(surface, query); frame = shown(surface, 100).join("\n"); surface.handleInput("\u001b"); });
    expect([query, frame.includes(id), frame.includes(other)]).toEqual([query, true, false]);
  }
});

test("current, live and ended rows stay listed but cannot be chosen", async () => {
  const rows = resumeRows(LIST, "A-1");
  for (const [query, id] of [["current work", "A-1"], ["elsewhere", "A-4"], ["T-12", "A-3"]] as const) {
    let frame = "";
    // Enter on such a row picks nothing, so the later Esc is what settles the promise.
    const picked = await drive(rows, surface => {
      type(surface, query);
      frame = shown(surface, 100).join("\n");
      surface.handleInput("\r");
      surface.handleInput("\u001b");
    });
    expect([id, picked, frame.includes(id)]).toEqual([id, undefined, true]);
  }
});

test("arrow keys move the selection and a second click chooses", async () => {
  const rows = resumeRows(LIST, "A-1");
  expect(await drive(rows, surface => { surface.handleInput("\u001b[B"); surface.handleInput("\r"); })).toBe(rows[1]!);
  // A click maps through the rows as last drawn, so the driver renders first and finds the session's row where it is.
  const clicked = await drive(rows, surface => {
    const at = () => shown(surface, 100).findIndex(line => new RegExp(`^│.A-2\\s`).test(line));
    surface.handleMouse?.(click(at()));
    expect(shown(surface, 100)[at()]).toMatch(/^│▌A-2\s/);
    surface.handleMouse?.(click(at()));
  });
  expect(clicked).toBe(rows[1]!);
});

test("a selected row keeps its whole title and exact time however long, and no row is wider than its width", () => {
  for (const summary of ["x".repeat(500), "会".repeat(200)]) {
    const [entry] = resumePopupRows(resumeRows([{ ...LIST[1], summary }], "A-1"));
    for (const width of [76, 40]) {
      const lines = entry!.lines(width, true).map(line => stripTerminalSequences(line));
      for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
      const text = lines.join("\n");
      expect(text).toContain("2026-11-04T09:00:00Z");
      expect(text.replace(/[^x会]/g, "").length).toBeGreaterThanOrEqual(summary.length);
    }
  }
});
