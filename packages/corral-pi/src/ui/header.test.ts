import { expect, test } from "bun:test";
import { sliceByColumn, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { HostIdentity } from "../roles/types.ts";
import { changesBadge, headerRows, mainTabs, tabRow, tabSpec, type TabId, type TabSpec } from "./header.ts";

const worker: HostIdentity = { store: "s", project: "demo", role: "worker", task: "T-1", run: "R-1", generation: 1 };
const reviewer: HostIdentity = { store: "s", project: "demo", role: "reviewer", task: "T-1", review: "V-1", generation: 1 };
const assistant: HostIdentity = { store: "s", project: "", role: "assistant", assistant: "A-1", generation: 1 };
const pm: HostIdentity = { store: "s", project: "demo", role: "pm", generation: 1 };

const BADGES: Partial<Record<TabId, TabSpec["badge"]>> = {
  conversation: { text: "●", tone: "accent" }, agents: { text: "12", tone: "accent" },
  changes: { text: "148", tone: "text" }, evidence: { text: "✓R·", tone: "warning" }, inbox: { text: "3", tone: "warning" },
};
const specs = (identity: HostIdentity) => mainTabs(identity).map(id => tabSpec(id, BADGES[id]));

test("the tab row never overflows, keeps the open tab's label and opens the tab under the pointer", () => {
  for (const identity of [worker, assistant]) {
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

test("task-bound roles get Task and Proof, assistant and PM neither, and Inbox is everywhere", () => {
  for (const identity of [worker, reviewer]) {
    expect(mainTabs(identity)).toContain("task"); expect(mainTabs(identity)).toContain("evidence");
  }
  for (const identity of [assistant, pm]) {
    expect(mainTabs(identity)).not.toContain("task"); expect(mainTabs(identity)).not.toContain("evidence");
  }
  for (const identity of [worker, reviewer, assistant, pm]) expect(mainTabs(identity)).toContain("inbox");
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
