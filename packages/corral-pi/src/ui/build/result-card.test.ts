import { expect, test } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { BuildSummary, ExitOption, ExitState } from "../../build/types.ts";
import { agentColor } from "../agent-hues.ts";
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
  const flags = text.filter(line => line.startsWith("!") && line.includes("client.ts"));
  expect(flags).toHaveLength(1);
  expect(flags[0]).toContain("picker-ui");
  expect(flags[0]).toContain("tests");
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

test("a failed exit shows its error detail with a cross", () => {
  const exit: ExitState = { phase: "failed", record: { choice: "ship", at: "t" }, detail: "The gate failed for T-962: just check" };
  const line = card({ summary: summary(), exit }).text.find(row => row.includes("The gate failed"));
  expect(line).toBeDefined();
  expect(line?.startsWith("✗")).toBeTrue();
});

test("slice rows and the pending exit row are clickable, and every line fits", () => {
  for (const width of [30, 52, 80, 140]) {
    const { lines, hits, text } = card({
      summary: summary({ dirty: true, outside: [{ path: "a/very/long/path/to/a/file/that/needs/clipping.ts", by: ["a", "b", "c"] }] }),
      note: "Not checked: the full gate (just check), a browser run of the arrival screen.",
    }, width);
    for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    const slice = text.findIndex(row => row.includes("host-slots"));
    expect(hits.get(slice)).toEqual({ child: "host-slots" });
    expect(hits.get(text.length - 1)).toEqual({ exit: true });
  }
});
