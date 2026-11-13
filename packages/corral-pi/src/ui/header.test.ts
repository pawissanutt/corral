import { expect, test } from "bun:test";
import { sliceByColumn, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { boardBadge, changesBadge, headerFrame, headerRows, mainTabs, tabRow, tabSpec, type HeaderInput, type TabId, type TabSpec } from "./header.ts";

const worker: HostIdentity = { store: "s", project: "demo", role: "worker", task: "T-1", run: "R-1", generation: 1 };
const reviewer: HostIdentity = { store: "s", project: "demo", role: "reviewer", task: "T-1", review: "V-1", generation: 1 };
const assistant: HostIdentity = { store: "s", project: "", role: "assistant", assistant: "A-1", generation: 1 };
const pm: HostIdentity = { store: "s", project: "demo", role: "pm", generation: 1 };
const build: HostIdentity = { store: "s", project: "demo", role: "build", generation: 1 };

const BADGES: Partial<Record<TabId, TabSpec["badge"]>> = {
  conversation: { text: "●", tone: "accent" }, agents: { text: "12", tone: "accent" },
  changes: { text: "148", tone: "text" }, evidence: { text: "✓R·", tone: "warning" }, inbox: { text: "3", tone: "warning" },
  board: { text: "3", tone: "accent" },
};
const specs = (identity: HostIdentity) => mainTabs(identity).map(id => tabSpec(id, BADGES[id]));

test("the tab row never overflows, keeps the open tab's label and opens the tab under the pointer", () => {
  for (const identity of [worker, assistant, build]) {
    const tabs = specs(identity);
    for (const width of [30, 34, 49, 50, 64, 104, 140]) for (const active of [...tabs.map(tab => tab.id), undefined]) {
      const { line, hits } = tabRow(tabs, active, width);
      expect(visibleWidth(line)).toBe(width);
      const text = stripTerminalSequences(line);
      const open = tabs.find(tab => tab.id === active);
      if (open) expect(text.includes(open.label) || (open.short !== undefined && text.includes(open.short))).toBeTrue();
      expect(hits.length).toBeGreaterThan(0);
      for (const hit of hits) {
        const tab = tabs.find(item => item.id === hit.id)!;
        expect(hit.to).toBeLessThanOrEqual(width);
        expect(sliceByColumn(text, hit.from, hit.to - hit.from)).toContain(tab.key);
      }
      if (open) expect(hits.some(hit => hit.id === open.id)).toBeTrue();
    }
  }
});

test("task-bound roles get Task and Proof, assistant and PM neither, and Inbox is everywhere but build", () => {
  for (const identity of [worker, reviewer]) {
    expect(mainTabs(identity)).toContain("task"); expect(mainTabs(identity)).toContain("evidence");
  }
  for (const identity of [assistant, pm]) {
    expect(mainTabs(identity)).not.toContain("task"); expect(mainTabs(identity)).not.toContain("evidence");
  }
  for (const identity of [worker, reviewer, assistant, pm]) expect(mainTabs(identity)).toContain("inbox");
});

test("a build session has Chat, Agents, Changes and Board, with Board on F10", () => {
  expect(mainTabs(build)).toEqual(["conversation", "agents", "changes", "board"]);
  expect(tabSpec("board")).toMatchObject({ key: "F10", label: "Board" });
});

test("the Board badge counts open tasks, warns on blocked or failed, and shows ◆ for a plan with nothing filed", () => {
  expect(boardBadge({ open: 3, alert: false, proposed: false })).toEqual({ text: "3", tone: "accent" });
  expect(boardBadge({ open: 3, alert: true, proposed: false })?.tone).toBe("warning");
  expect(boardBadge({ open: 0, alert: false, proposed: true })).toEqual({ text: "◆", tone: "warning" });
  expect(boardBadge({ open: 2, alert: false, proposed: true })?.text).toBe("2");
  expect(boardBadge({ open: 0, alert: false, proposed: false })).toBeUndefined();
});

test("a failed Git read or a missing project reads ! or ?, and the Changes badge never reads 0", () => {
  const failed = changesBadge({ checked: true, files: 0, scopeRequired: false, error: true });
  const choose = changesBadge({ checked: false, files: 0, scopeRequired: true, error: false });
  expect(failed?.text).toBe("!"); expect(choose?.text).toBe("?");
  expect(changesBadge({ checked: false, files: 0, scopeRequired: false, error: false })).toBeUndefined();
  expect(changesBadge({ checked: true, files: 0, scopeRequired: false, error: false })).toBeUndefined();
  expect(changesBadge({ checked: true, files: 4, scopeRequired: false, error: false })?.text).toBe("4");
  const row = stripTerminalSequences(tabRow([tabSpec("conversation"), tabSpec("changes", failed)], "conversation", 104).line);
  expect(row).toMatch(/Changes\s+!/); expect(row).not.toMatch(/Changes\s+0/);
});

test("the header keeps one row when short or tiny and two otherwise, each exactly the width", () => {
  const input = { title: "T-1 · R-1", state: "Working" as const, attention: { text: "⚑ 2", tone: "warning" as const },
    context: { left: "Build the slots screen", right: "running · P1 · xs" } };
  for (const width of [30, 63, 64, 104, 150]) {
    const tiny = width < 64;
    for (const short of [false, true]) {
      const rows = headerRows(input, width, { short, tiny });
      expect(rows).toHaveLength(short || tiny ? 1 : 2);
      for (const row of rows) expect(visibleWidth(row)).toBe(width);
    }
  }
});

test("header hits cover the attention chip and a clickable status where they are drawn", () => {
  const status = { long: "exit choice pending", short: "choose exit", tone: "warning" as const, clickable: true };
  for (const [width, tiny] of [[140, false], [60, true], [60, false]] as const) {
    const input: HeaderInput = {
      title: "build · save-slots", state: "Idle", status,
      attention: { text: tiny ? "▲ 1" : "▲ 1 needs you", tone: "warning" },
      context: { left: "mitiplay · .worktrees/build-save-slots", right: "exit policy: gate allowed" },
    };
    const { lines, hits } = headerFrame(input, width, { short: false, tiny });
    for (const line of lines) expect(visibleWidth(line)).toBe(width);
    const text = stripTerminalSequences(lines[0]!);
    const cells = (id: "attention" | "status") => {
      const hit = hits.find(item => item.id === id)!;
      expect(hit.row).toBe(0);
      expect(hit.to).toBeLessThanOrEqual(width);
      return sliceByColumn(text, hit.from, hit.to - hit.from).trim();
    };
    expect(cells("attention")).toBe(input.attention!.text);
    expect([status.long, status.short]).toContain(cells("status"));
    if (tiny) expect(cells("status")).toBe(status.short);
  }
  const quiet = headerFrame({ title: "build · save-slots", state: "Idle", status: { ...status, clickable: false } }, 140, { short: false, tiny: false });
  expect(quiet.hits).toHaveLength(0);
  expect(stripTerminalSequences(quiet.lines[0]!)).toContain(status.long);
});

test("a notice still replaces the status for its duration and takes away its hit", () => {
  const input: HeaderInput = {
    title: "build · save-slots", state: "Idle", notice: { text: "Steered picker-ui", tone: "success" },
    status: { long: "exit choice pending", short: "choose exit", tone: "warning", clickable: true },
  };
  const { lines, hits } = headerFrame(input, 140, { short: false, tiny: false });
  const text = stripTerminalSequences(lines[0]!);
  expect(text).toContain("Steered picker-ui");
  expect(text).not.toContain("exit choice pending");
  expect(hits.some(hit => hit.id === "status")).toBeFalse();
  expect(headerRows(input, 140, { short: false, tiny: false })).toEqual(lines);
});
