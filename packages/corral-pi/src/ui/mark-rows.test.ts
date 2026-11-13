import { expect, test } from "bun:test";
import type { CustomEntry, EntryRenderer } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";

import { MARK_ENTRY } from "../host/transcript-marks.ts";
import type { TranscriptMark } from "../host/transcript-marks.ts";
import { installMarkRows } from "./mark-rows.ts";

const AT = new Date(2026, 10, 13, 14, 35).getTime();

/** Every mark through the registered renderer, as Pi draws an appended entry: plain rows at `width`. */
function drawn(marks: readonly TranscriptMark[], width: number, expanded = false): string[][] {
  let renderer: EntryRenderer | undefined;
  const entries: CustomEntry[] = [];
  const sink = installMarkRows({
    registerEntryRenderer: (type: string, draw: EntryRenderer) => { if (type === MARK_ENTRY) renderer = draw; },
    appendEntry: (customType: string, data: unknown) => { entries.push({ type: "custom", customType, data, id: `e${entries.length}`, parentId: null, timestamp: "" }); },
  } as never);
  for (const mark of marks) sink.add(mark);
  return entries.map(entry => (renderer?.(entry, { expanded } as never, undefined as never)?.render(width) ?? []).map(line => stripTerminalSequences(line)));
}
const one = (mark: TranscriptMark, width = 100) => drawn([mark], width)[0]!.join("\n");

test("a waiting run names what it waits for and counts the rest", () => {
  const waits = [{ kind: "gate", id: "ag-1", label: "gate quick on T-950/R2" }, { kind: "job", id: "j1-4", label: "job j1-4" }] as const;
  const row = one({ kind: "turn_end", reason: "waiting", at: AT, durationMs: 64_000, toolCalls: 5, waits });
  expect(row).toContain("waiting for gate quick on T-950/R2");
  expect(row).toContain("+ 1 more");
  // Expanded lists every wait.
  expect(drawn([{ kind: "turn_end", reason: "waiting", at: AT, durationMs: 64_000, toolCalls: 5, waits }], 100, true)[0]!.join("\n")).toContain("job j1-4");
});

test("a stopped or failed run's mark names no cause: the assistant message above does", () => {
  for (const reason of ["stopped", "error"] as const) {
    const row = drawn([{ kind: "turn_end", reason, at: AT, durationMs: 12_000, toolCalls: 3, notRun: 2, note: "the run budget was reached", waits: [] }], 100, true)[0]!.join("\n");
    expect(row).toContain("3 tool calls");
    expect(row).not.toContain("by you");
    expect(row).not.toContain("did not run");
    expect(row).not.toContain("budget");
  }
});

test("cost shows only when the host recorded it", () => {
  const base = { kind: "turn_end", reason: "done", at: AT, durationMs: 192_000, toolCalls: 14, waits: [] } as const;
  expect(one(base)).toContain("14 tool calls");
  expect(one(base)).not.toContain("$");
  expect(one({ ...base, cost: 0.42 })).toContain("$0.42");
});

test("a steer names the child it went to", () => {
  const row = one({ kind: "child", at: AT, act: "steered", child: "picker-ui", text: "use the existing Button" }, 46);
  expect(row).toMatch(/^ → picker-ui steered: /u);
});

test("every mark is one row collapsed and never wider than the terminal", () => {
  const marks: TranscriptMark[] = [
    { kind: "turn_end", reason: "continue_stopped", at: AT, durationMs: 310_000, toolCalls: 22, cost: 1.3, rebilledTokens: 120_000, note: "no progress on plan P-3 (3 steps open)", waits: [] },
    { kind: "turn_end", reason: "error", at: AT, durationMs: 4200, toolCalls: 1, note: "the provider refused the request", waits: [] },
    { kind: "model", at: AT, model: "gpt-6.1-sol", effort: "high", was: { model: "claude-opus-5", effort: "medium" } },
    { kind: "note", at: AT, tone: "warning", text: "corral: herdr is not reachable\nthe agent keeps working without it" },
    { kind: "child", at: AT, act: "asked", child: "picker-ui", text: "With zero slots, what should the arrival screen show?" },
  ];
  for (const width of [100, 61, 46]) {
    for (const rows of drawn(marks, width)) {
      expect(rows).toHaveLength(1);
      expect(visibleWidth(rows[0]!)).toBeLessThanOrEqual(width);
    }
  }
});
