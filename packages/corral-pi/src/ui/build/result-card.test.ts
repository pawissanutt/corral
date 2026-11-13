import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
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
  { choice: "keep", label: "Keep on the branch", detail: "d" }, { choice: "land", label: "Land after the gate", detail: "d" },
];
function card(input: { summary?: BuildSummary; exit?: ExitState; live?: boolean; note?: string }, width = 100) {
  const result = resultCardLines({
    summary: input.summary, fleet: undefined, exit: input.exit ?? { phase: "pending" }, options, live: input.live ?? true,
    ...(input.note ? { note: input.note } : {}),
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
  expect(text.join("\n")).not.toContain("Exit choice pending");
  expect(text.join("\n")).not.toContain("Reading the branch");
});

test("uncommitted changes are called out", () => {
  expect(card({ summary: summary({ dirty: true }) }).text.join("\n")).toContain("Uncommitted changes");
  expect(card({ summary: summary() }).text.join("\n")).not.toContain("Uncommitted changes");
});

test("a card that is not the latest says it was superseded and shows no live progress", () => {
  const exit: ExitState = { phase: "landing", record: { choice: "land", at: "t", task: "T-962" }, detail: "Checking whether T-962 has landed." };
  const old = card({ summary: undefined, exit, live: false }).text.join("\n");
  expect(old).toContain("superseded");
  expect(old).not.toContain("T-962");
  expect(old).not.toContain("you chose");
  expect(old).not.toContain("Reading the branch");
  const latest = card({ summary: summary(), exit, live: true }).text.join("\n");
  expect(latest).toContain("Land after the gate");
  expect(latest).toContain("T-962");
});

test("a failed exit without a record shows its error detail with a cross on the card", () => {
  const exit: ExitState = { phase: "failed", detail: "The gate failed for T-962: just check" };
  const line = card({ summary: summary(), exit }).text.find(row => row.includes("The gate failed"));
  expect(line).toBeDefined();
  expect(line).toContain("✗ The gate failed");
});

test("a settled exit shows its choice and outcome once across the card and its entry, a running exit stays on the card", () => {
  const kept: ExitState = { phase: "kept", record: { choice: "keep", at: "t" }, detail: "The worktree and branch are kept without a corral task." };
  const entry = exitEntry({ choice: "keep", label: "Keep on the branch", detail: "d" }, kept);
  const drawn = buildEntryRenderer(() => undefined)({ role: "custom", ...entry, timestamp: NOW }, { expanded: false, outputPad: 1 } as never, undefined as never);
  const both = [...card({ summary: summary(), exit: kept }).text, ...(drawn?.render(100) ?? []).map(line => stripTerminalSequences(line))];
  expect(both.filter(line => line.includes("you chose"))).toHaveLength(1);
  expect(both.filter(line => line.includes("kept without a corral task"))).toHaveLength(1);
  const running: ExitState = { phase: "running", record: { choice: "land", at: "t" }, detail: "Submitting the branch." };
  const live = card({ summary: summary(), exit: running }).text.join("\n");
  expect(live).toContain("you chose");
  expect(live).toContain("● Submitting the branch.");
});

test("slice rows and, while the exit is pending, the header are clickable, and every line fits", () => {
  for (const width of [30, 46, 52, 61, 80, 92, 140, 185]) {
    const { lines, hits, text } = card({
      summary: summary({ dirty: true, outside: [{ path: "a/very/long/path/to/a/file/that/needs/clipping.ts", by: ["a", "b", "c"] }] }),
      note: "Not checked: the full gate (just check), a browser run of the arrival screen.",
    }, width);
    for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const slice = text.findIndex(row => row.includes("host-slots"));
    expect(hits.get(slice)).toEqual({ child: "host-slots" });
    expect(hits.get(1)).toEqual({ exit: true });
  }
  expect(card({ summary: summary(), exit: { phase: "none" } }).hits.get(1)).toBeUndefined();
});
