import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { BuildRecord } from "../../build/record-types.ts";
import type { BuildSummary, ExitOption, ExitState } from "../../build/types.ts";
import { agentColor } from "../agent-hues.ts";
import { buildEntryRenderer, exitEntry } from "./build-entries.ts";
import { resultCardLines } from "./result-card.ts";

const NOW = 3_000_000;
const summary = (extra: Partial<BuildSummary> = {}): BuildSummary => ({
  branch: "build/save-slots", base: "abc", head: "def", commits: 4, files: 9, added: 475, removed: 11, dirty: false,
  slices: [
    { id: "host-slots", state: "done", seconds: 90, cost: 0.21 },
    { id: "picker-ui", state: "failed", seconds: 40 },
  ],
  outside: [], ...extra,
});
const options: ExitOption[] = [
  { choice: "keep", label: "Keep on the branch", detail: "d" }, { choice: "land", label: "Land on main", detail: "d" },
];
type Gates = BuildRecord["verification"];
const GATES: Gates = [
  { attempt: "G1", head: "aaa", result: "fail", failed_command: "just check-unit", started: "2026-01-01T00:00:00Z", ended: "2026-01-01T00:01:30Z" },
  { attempt: "G2", head: "bbb", result: "pass", started: "2026-01-01T00:02:00Z", ended: "2026-01-01T00:12:00Z" },
];
const REVIEWS: BuildRecord["reviews"] = [{
  round: 1, reviewer: "review-1", model: "anthropic/claude-opus-5.5", head: "bbb", verdict: "changes", at: "2026-01-01T00:13:00Z",
  findings: [
    { id: "F1", severity: "major", path: "src/slots.rs", line: 42, text: "OPEN_FINDING_TEXT" },
    { id: "F2", severity: "minor", text: "FIXED_FINDING_TEXT", resolution: { state: "fixed", commit: "ccc" } },
  ],
}];
function card(input: { summary?: BuildSummary; exit?: ExitState; live?: boolean; note?: string; gates?: Gates; reviews?: BuildRecord["reviews"]; since?: number }, width = 100) {
  const result = resultCardLines({
    summary: input.summary, fleet: undefined, exit: input.exit ?? { phase: "pending" }, options, live: input.live ?? true,
    record: { verification: input.gates ?? [], reviews: input.reviews ?? [] },
    ...(input.note ? { note: input.note } : {}), ...(input.since === undefined ? {} : { exitSince: input.since }),
  }, width, agentColor, NOW);
  return { ...result, text: result.lines.map(line => stripTerminalSequences(line)) };
}

test("a file edited outside their files by two children is one flag row naming both", () => {
  const { text } = card({ summary: summary({ outside: [{ path: "app/src/api/client.ts", by: ["picker-ui", "tests"] }] }) });
  const flags = text.filter(line => line.includes("! ") && line.includes("client.ts"));
  expect(flags).toHaveLength(1);
  expect(flags[0]).toContain("picker-ui");
  expect(flags[0]).toContain("tests");
});

test("the card is a rounded box headed by the build, slice rows in the sres columns, then the branch figures", () => {
  const { text } = card({ summary: summary(), note: "Not checked: the full gate." }, 140);
  expect(text[0]?.startsWith("╭")).toBeTrue();
  expect(text[text.length - 1]?.startsWith("╰")).toBeTrue();
  expect(visibleWidth(text[0] ?? "")).toBe(108);
  expect(text[1]).toContain("Build finished · 1/2 slices");
  // Name in 11 cells, the state and time in 9, two cells apart, then the cost.
  const row = text[2] ?? "";
  expect(row.indexOf("✓ 1m30s")).toBe(row.indexOf("host-slots") + 13);
  expect(row.indexOf("$0.21")).toBe(row.indexOf("host-slots") + 24);
  expect(text[4]).toContain("9 files +475 −11 on build/save-slots, 4 commits");
  expect(text.join("\n")).toContain("Not checked: the full gate.");
});

test("uncommitted changes are called out", () => {
  expect(card({ summary: summary({ dirty: true }) }).text.join("\n")).toContain("Uncommitted changes");
  expect(card({ summary: summary() }).text.join("\n")).not.toContain("Uncommitted changes");
});

test("gate attempts and review rounds draw their blocks with each open finding; an empty record draws neither", () => {
  for (const width of [46, 80, 140]) {
    const { text } = card({ summary: summary(), gates: GATES, reviews: REVIEWS }, width);
    const gates = ["G1", "G2"].map(attempt => text.findIndex(line => line.includes(`│ ${attempt} `)));
    expect(gates.every(index => index > 0)).toBeTrue();
    expect(text[gates[0]!]).toContain("✗");
    expect(text[gates[1]!]).toContain("✓");
    const round = text.findIndex(line => line.includes("R1"));
    const finding = text.findIndex(line => line.includes("src/slots.rs:42"));
    expect(round).toBeGreaterThan(gates[1]!);
    expect(finding).toBeGreaterThan(round);
    // A resolved finding collapses to a count; the open one keeps its row.
    expect(text.join("\n")).not.toContain("FIXED_FINDING_TEXT");
  }
  const empty = card({ summary: summary() }).text;
  expect(empty.some(line => line.includes("R1") || line.includes("G1"))).toBeFalse();
});

test("a card that is not the latest says it was superseded and shows neither live progress nor the record", () => {
  const exit: ExitState = { phase: "verifying", record: { choice: "land", at: "t" }, detail: "Running just check on 3f2a9c1." };
  const old = card({ summary: undefined, exit, live: false, gates: GATES, reviews: REVIEWS }).text.join("\n");
  expect(old).toContain("superseded");
  expect(old).not.toContain("3f2a9c1");
  expect(old).not.toContain("you chose");
  expect(old).not.toContain("src/slots.rs");
  const latest = card({ summary: summary(), exit, live: true, since: NOW - 125_000 }).text.join("\n");
  expect(latest).toContain("Land on main");
  expect(latest).toContain("3f2a9c1");
  expect(latest).toContain("2m05s");
});

test("a failed exit without a record shows its error detail with a cross on the card", () => {
  const exit: ExitState = { phase: "failed", detail: "gate_failed: just check" };
  const line = card({ summary: summary(), exit }).text.find(row => row.includes("gate_failed"));
  expect(line).toContain("✗ gate_failed");
});

test("a settled exit shows its choice and outcome once across the card and its entry; a landing exit stays on the card", () => {
  const landed: ExitState = { phase: "landed", record: { choice: "land", at: "t", landed: "3f2a9c1e0d", baseBranch: "main", gateAttempt: "G2" } };
  const entry = exitEntry({ choice: "land", label: "Land on main", detail: "d" }, landed);
  const drawn = buildEntryRenderer(() => undefined)({ role: "custom", ...entry, timestamp: NOW }, { expanded: false, outputPad: 1 } as never, undefined as never);
  const both = [...card({ summary: summary(), exit: landed }).text, ...(drawn?.render(100) ?? []).map(line => stripTerminalSequences(line))];
  expect(both.filter(line => line.includes("you chose"))).toHaveLength(1);
  expect(both.filter(line => line.includes("3f2a9c1") && line.includes("main"))).toHaveLength(1);
  for (const phase of ["rebasing", "verifying", "landing"] as const) {
    const live = card({ summary: summary(), exit: { phase, record: { choice: "land", at: "t" } } }).text.join("\n");
    expect(live).toContain("you chose");
  }
});

test("slice rows and, while the exit is pending, the header are clickable, and every line fits", () => {
  for (const width of [30, 46, 52, 61, 80, 92, 140, 185]) {
    const { lines, hits, text } = card({
      summary: summary({ dirty: true, outside: [{ path: "a/very/long/path/to/a/file/that/needs/clipping.ts", by: ["a", "b", "c"] }] }),
      note: "Not checked: the full gate (just check), a browser run of the arrival screen.", gates: GATES, reviews: REVIEWS,
    }, width);
    for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const slice = text.findIndex(row => row.includes("host-slots"));
    expect(hits.get(slice)).toEqual({ child: "host-slots" });
    expect(hits.get(1)).toEqual({ exit: true });
  }
  expect(card({ summary: summary(), exit: { phase: "none" } }).hits.get(1)).toBeUndefined();
});
