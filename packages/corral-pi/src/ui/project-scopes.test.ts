import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { stripTerminalSequences, Text, visibleWidth } from "@earendil-works/pi-tui";
import { renderLayoutFrame } from "../../node_modules/@earendil-works/pi-tui/dist/layout.js";
import { chooseProject, projectScopes } from "./project-scopes.ts";
import { NativePanels } from "./panels.ts";
import { ReviewStore } from "./review-store.ts";
import type { PanelSnapshot } from "./panels-view.ts";
import { RecordingTerminal } from "../../test/support/panel-terminal.ts";

const identity = { store: "st-0123456789abcdef", project: "", role: "assistant" as const, assistant: "A-22", generation: 1 };
const snapshot: PanelSnapshot = { bound: true, model: "provider/model", effort: "high", streaming: false, context: undefined,
  tokens: 0, tools: [], contextFiles: [], skills: [], jobs: [], auxiliary: null, steering: 0, followUps: 0, ledgerStale: false,
  ledger: { plan: "P3", revision: 9, spec_revision: "spec", phases: [{ id: "ui", label: "Project panels", steps: [
    { id: "implement", label: "Implement project-aware panels", status: { state: "in_progress" }, criteria: [] },
    { id: "check", label: "Verify", status: { state: "pending" }, criteria: [] },
  ] }], waits: [{ step: "check", on: { kind: "owner", question: "Review the project-specific changes and confirm the exact candidate before landing it." } }] },
};

test("project choices resolve configured paths and include only this assistant's supplied scratches", async () => {
  const scopes = projectScopes({ workspace: "/workspace", projects: { alpha: { path: "alpha" }, beta: { path: "/other/beta" } },
    session: { scratch: [{ project: "alpha", slug: "panels", path: "/workspace/alpha/.worktrees/A-22-panels" }] } });
  expect(scopes).toEqual([{ label: "alpha", path: "/workspace/alpha" }, { label: "beta", path: "/other/beta" },
    { label: "alpha / panels (scratch)", path: "/workspace/alpha/.worktrees/A-22-panels" }]);
  expect(await chooseProject(scopes, async (_title, choices) => choices[2])).toBe(scopes[2]);
  expect(await chooseProject(scopes, async () => undefined)).toBeUndefined();
  expect(() => projectScopes({ workspace: "relative", projects: {} })).toThrow();
  expect(() => projectScopes({ workspace: "/workspace", projects: { bad: { path: 2 } } })).toThrow();
});

test("picker sanitizes labels without changing paths or confusing duplicate labels", async () => {
  const scopes = [{ label: "alpha\x1b[2J", path: "/repo/one" }, { label: "alpha [2J", path: "/repo/two" }];
  expect(await chooseProject(scopes, async (_title, choices) => {
    expect(choices.every(label => !label.includes("\x1b"))).toBeTrue();
    expect(new Set(choices).size).toBe(2);
    return choices[1];
  })).toBe(scopes[1]);
});

test("workspace Git is not read before choosing a project; switching never leaks old paths or patch metadata", async () => {
  const root = mkdtempSync(join(tmpdir(), "panel-projects-"));
  const git = (cwd: string, ...args: string[]) => execFileSync("git", ["-C", cwd, "-c", "core.hooksPath=/dev/null", ...args], { encoding: "utf8" });
  const store = new ReviewStore(root, () => {}, undefined, true);
  try {
    git(root, "init", "-q"); writeFileSync(join(root, "workspace-only.txt"), "unrelated\n");
    const alpha = join(root, "alpha"), beta = join(root, "beta");
    for (const repo of [alpha, beta]) { mkdirSync(repo); git(repo, "init", "-q"); }
    writeFileSync(join(alpha, "alpha-only.ts"), "alpha\n"); writeFileSync(join(beta, "beta-only.ts"), "beta\n");
    await store.refresh(); expect(store.files).toEqual([]); expect(store.checkedAt).toBeUndefined();
    await store.setRepository(alpha);
    expect(store.scopeRequired).toBeFalse(); expect(store.files.map(file => file.path)).toEqual(["alpha-only.ts"]);
    expect(store.patchSnapshot?.root).toBe(alpha); expect(store.patch).toContain("+alpha");
    const pending = store.select("alpha-only.ts");
    rmSync(join(alpha, "alpha-only.ts"));
    const comparison = store.setComparison("staged");
    expect(store.patchSnapshot).toBeUndefined(); expect(store.patchPath).toBeUndefined();
    await pending; await comparison;
    expect(store.patchComparison).toBe("staged"); expect(store.patchPath).toBeUndefined();
    store.query = "alpha";
    await store.setRepository(beta);
    expect(store.query).toBe(""); expect(store.files.map(file => file.path)).toEqual(["beta-only.ts"]);
    expect(store.patchSnapshot?.root).toBe(beta); expect(store.patch).toContain("+beta"); expect(store.patch).not.toContain("alpha");
    const nested = join(root, "not-a-repo"); mkdirSync(nested);
    const initial = new ReviewStore(nested, () => {}, undefined, false, true);
    try {
      await initial.refresh();
      expect(initial.files).toEqual([]); expect(initial.error).toContain("parent workspace changes are not shown");
    } finally { initial.dispose(); }
    await store.setRepository(nested);
    expect(store.files).toEqual([]); expect(store.error).toContain("parent workspace changes are not shown");
    expect(store.comparisons).not.toContain("candidate");
    await store.setComparison("candidate");
    expect(store.comparison).toBe("session");
    await store.setRepository(join(root, "missing"));
    expect(store.files).toEqual([]); expect(store.patchSnapshot).toBeUndefined(); expect(store.error).toBeDefined();
  } finally { store.dispose(); rmSync(root, { recursive: true, force: true }); }
});

test("project-less assistant shows actionable scope choice, not a misleading clean workspace", async () => {
  const panels = new NativePanels({ identity, cwd: tmpdir(), readProjects: async () => ({}), snapshot: () => snapshot });
  try {
    panels.wrap(new RecordingTerminal());
    await panels.loadFiles();
    expect(panels.workbench.review.checkedAt).toBeUndefined();
    expect(panels.workbench.sideSource().files.state).toBe("choose");
    panels.sidebar.tab = "files";
    const files = stripTerminalSequences(panels.renderSidebar(34, 26).join("\n"));
    expect(files).toContain("No project chosen"); expect(files).toContain("Choose project"); expect(files).not.toContain("No changes yet");
    // The Changes tab reads `?`, never a count of zero.
    const root = panels.workbench.mount(new Text("transcript", 0, 0));
    const tabs = stripTerminalSequences(renderLayoutFrame(root, 104, 24, () => {}).lines.join("\n"));
    expect(tabs).toMatch(/Changes\s+\?/); expect(tabs).not.toMatch(/Changes\s+0/);
    panels.sidebar.tab = "plan";
    const plan = stripTerminalSequences(panels.renderSidebar(34, 26).join("\n"));
    expect(plan).toContain("0/2 done"); expect(plan).toContain("Implement project-aware panels");
  } finally { panels.dispose(); }
});

test("the rail Plan tab marks waiting steps and keeps every row the rail's width", () => {
  const panels = new NativePanels({ identity, cwd: tmpdir(), snapshot: () => snapshot });
  const physical = new RecordingTerminal(); physical.resize(140, 18);
  const terminal = panels.wrap(physical); terminal.start(() => {}, () => {});
  try {
    panels.sidebar.tab = "plan";
    for (const height of [8, 18]) {
      const lines = panels.renderSidebar(34, height);
      expect(lines).toHaveLength(height);
      for (const line of lines) expect(visibleWidth(line)).toBe(34);
    }
    const text = stripTerminalSequences(panels.renderSidebar(34, 18).join("\n"));
    expect(text).toMatch(/Verify\s+\?/);
  } finally { terminal.stop(); panels.dispose(); }
});
