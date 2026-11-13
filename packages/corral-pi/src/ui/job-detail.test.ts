import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import { jobDetailLines, numberedTail, pagerLines } from "./job-detail.ts";
import type { PanelJob } from "./panels-view.ts";

const NOW = 1_000_000;
const tail = Array.from({ length: 30 }, (_, index) => `line ${index + 1}`).join("\n") + "\n";
const running: PanelJob = { job: "j-1", state: "running", detail: { job: "j-1", command: "cargo test", state: "running", startedAt: NOW - 8000,
  endedAt: undefined, exitCode: null, signal: null, groupClear: true, bytes: 10, truncated: false, tail, lines: 30, monitorAttempts: 0, watches: [] } };
const input = { bound: true, now: NOW, legend: false, freeze: undefined };

test("retained tail numbers end at the host line count, with the open line while running", () => {
  expect(numberedTail("a\nb\n", 2, true).map(line => line.number)).toEqual([1, 2, 3]);
  expect(numberedTail("a\nb\n", 2, false).map(line => line.number)).toEqual([1, 2]);
  expect(numberedTail("a\nb", 2, false).map(line => line.number)).toEqual([1, 2]);
  expect(numberedTail("x\x1b[31mred\x1b[0m\rdone\n", 9, false)).toEqual([{ number: 9, text: "done" }]);
  expect(numberedTail("a\n", undefined, true).every(line => line.number === undefined)).toBe(true);
  const lines = jobDetailLines(running, input, 80, 24);
  for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(80);
  const plainRows = lines.map(line => stripTerminalSequences(line));
  // The retained tail sits in a rounded card whose header names it and counts the lines shown.
  expect(plainRows.some(row => row.startsWith("╭") && row.endsWith("╮"))).toBeTrue();
  expect(plainRows.some(row => /^│ output +last \d+ of 31 lines │$/.test(row))).toBeTrue();
  expect(plainRows[0]).toMatch(/^\$ cargo test {2}j-1/);
  const gutter = lines.map(line => stripTerminalSequences(line)).flatMap(row => /│ +(\d+) │/.exec(row)?.[1] ?? []).map(Number);
  expect(gutter.at(-1)).toBe(31);
  expect(gutter.every((value, index) => index === 0 || value === (gutter[index - 1] ?? 0) + 1)).toBe(true);
  expect(pagerLines(running).at(-1)).toMatch(/^31 │ $/);
});

test("the job card reports local monitor observations without claiming canonical success", () => {
  const watched: PanelJob = { job: "j1-1", state: "running", detail: { job: "j1-1", command: "build", state: "running", startedAt: 1000, endedAt: undefined,
    exitCode: null, signal: null, groupClear: false, bytes: 30, truncated: false, tail: "READY\n", monitorAttempts: 2,
    watches: [{ wait: "w1", pattern: "READY", armed: true, matched: "READY", reportQueued: true, deadline: new Date(5000).toISOString() }] } };
  const text = jobDetailLines(watched, { ...input, now: 3000, legend: true }, 100, 30).map(line => stripTerminalSequences(line)).join("\n");
  expect(text).toContain("2s"); expect(text).toContain("watch /READY/ matched");
  expect(text).toContain("settlement unknown"); expect(text).toContain("2 attempts (not active-wait count)");
  expect(text).not.toContain("100%");
  const frozen = jobDetailLines(running, { ...input, freeze: { job: "j-1", tail: "early\n", lines: 1, running: true } }, 80, 24)
    .map(line => stripTerminalSequences(line)).join("\n");
  expect(frozen).toContain("follow paused"); expect(frozen).not.toContain("line 30");
});
