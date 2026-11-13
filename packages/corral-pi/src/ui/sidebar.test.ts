import { test, expect } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import {
  Sidebar, drawerRows, type SideAgent, type SideFile, type SideFrame, type SideJob, type SideSource, type SideTab,
} from "./sidebar.ts";
import { PANEL_BG, PANEL_SELECTED, RESET } from "./panels-view.ts";
import type { TodoProgress } from "./todo-widget.ts";

const HUE = "\x1b[38;2;143;163;199m";
const source = (over: Partial<SideSource> = {}): SideSource => ({
  agents: [], jobs: [], files: { scope: "corral", state: "ok", list: [] }, plan: { progress: null, stale: false },
  bound: true, color: () => HUE, ...over,
});
const agent = (key: string, name: string, over: Partial<SideAgent> = {}): SideAgent =>
  ({ key, name, hue: 0, profile: "explore", glyph: "●", tone: "accent", activity: "read src/a.ts", ...over });
const job = (key: string, command: string, over: Partial<SideJob> = {}): SideJob =>
  ({ key, command, glyph: "●", tone: "accent", time: "12s", ...over });
const file = (path: string, letter = "M", added: number | null = 1, removed: number | null = 0): SideFile => ({ path, letter, added, removed });
const step = (id: string, state: "done" | "in_progress" | "pending") => ({ id, label: id, status: { state }, criteria: [] });
const plain = (frame: SideFrame) => frame.lines.map(stripTerminalSequences);
// Unique target keys; a two-line agent item carries its target on both lines.
const keys = (frame: SideFrame) => [...new Set([...frame.hits.values()].flat().flatMap(hit => hit.target && "key" in hit.target ? [hit.target.key] : []))];

test("failures stay visible while successful completions fold into one line", () => {
  const s = source({
    agents: [agent("helper:1", "scout"), agent("helper:2", "oracle", { glyph: "✓", tone: "success", settled: "ok" }),
      agent("helper:3", "shadow", { glyph: "!", tone: "warning", settled: "failed", activity: "exit 1" })],
    jobs: [job("job:1", "bun test"), job("job:2", "cargo build", { glyph: "✓", tone: "success", settled: "ok" }),
      job("job:3", "make lint", { glyph: "✗", tone: "error", settled: "failed" })],
  });
  const frame = new Sidebar(() => {}).rail(s, 34, 30), lines = plain(frame);
  expect(keys(frame).sort()).toEqual(["helper:1", "helper:3", "job:1", "job:3"]);
  expect(lines.some(line => line.includes("! shadow"))).toBeTrue();
  expect(lines.some(line => line.includes("✗ make lint"))).toBeTrue();
  const folded = lines.filter(line => line.includes("oracle"));
  expect(folded).toHaveLength(1);
  expect(folded[0]).toContain("cargo build");
  expect(lines.some(line => line.includes("COMPLETED · 2"))).toBeTrue();
});

test("waiting plan steps carry ? for the owner and … for an event, and done steps fold only under pressure", () => {
  const progress = { revision: 1, spec_revision: "s", phases: [{ id: "p", label: "Build", steps: [
    step("read", "done"), step("write", "done"), step("build", "in_progress"), step("ask", "pending"), step("land", "pending")] }],
  waits: [{ step: "ask", on: { kind: "owner", question: "ship?" } }, { step: "land", on: { kind: "event", event: "ci" } }] } as TodoProgress;
  const s = source({ plan: { progress, stale: false } }), bar = new Sidebar(() => {});
  bar.tab = "plan";
  const tall = plain(bar.rail(s, 34, 30)).map(line => line.trimEnd());
  expect(tall.some(line => line.includes("✓ read"))).toBeTrue();
  expect(tall.some(line => line.includes("✓ 2 done"))).toBeFalse();
  expect(tall.find(line => line.includes("○ ask"))).toMatch(/\?$/);
  expect(tall.find(line => line.includes("○ land"))).toMatch(/…$/);
  const short = plain(bar.rail(s, 34, 6)).map(line => line.trimEnd());
  expect(short.some(line => line.includes("✓ 2 done"))).toBeTrue();
  expect(short.some(line => line.includes("read"))).toBeFalse();
  expect(short.find(line => line.includes("● build"))).toBeDefined();
  expect(short.find(line => line.includes("○ ask"))).toMatch(/\?$/);
  expect(short.find(line => line.includes("○ land"))).toMatch(/…$/);
});

test("every rail, row and drawer line is exactly the width, wide glyphs included, with no black cells and no key hints", () => {
  const phases = [{ id: "a", label: "設計フェーズの長いラベル", steps: [step("s1", "done"), { ...step("s2", "in_progress"), label: "実装する 🙂 とても長い手順の説明文" }] },
    { id: "b", label: "Verify", steps: Array.from({ length: 9 }, (_, i) => step(`v${i}`, "pending")) }];
  const wide = source({
    bound: false,
    agents: [agent("helper:w", "検索エージェント長い名前", { profile: "探索", activity: "読み込み中 docs/設計書/とても長いファイル名.md 🙂🙂" }),
      ...Array.from({ length: 6 }, (_, i) => agent(`helper:${i}`, `explore ${i + 2}`, { activity: "x".repeat(60) })),
      agent("helper:ok", "oracle", { settled: "ok" })],
    jobs: [job("job:w", "bun test ✨ 全部のテストを実行する", { time: "1h05m" }), job("job:f", "y".repeat(70), { settled: "failed", glyph: "✗", tone: "error" })],
    files: { scope: "コーラル", state: "ok", list: [
      { ...file("src/ドキュメント/説明書.md", "A", 120, 4), who: [0, 1, 2], outside: true }, file("z".repeat(80), "?", null, null),
      ...Array.from({ length: 14 }, (_, i) => file(`packages/corral-pi/src/ui/deeply/nested/file-${i}.ts`, "M", 1000 + i, 999))] },
    plan: { progress: { revision: 1, spec_revision: "s", phases } as TodoProgress, stale: true },
  });
  const tabs: readonly SideTab[] = ["context", "files", "plan"];
  for (const width of [24, 34, 40]) for (const tab of tabs) for (const focused of [false, true]) {
    const bar = new Sidebar(() => {});
    bar.tab = tab; bar.focused = focused;
    bar.handleKey("j", wide);
    const frames = [bar.rail(wide, width, 20), bar.rail(wide, width, 6, 30), bar.row(wide, width, false), bar.row(wide, width, true),
      bar.drawerFrame(wide, width, 14), bar.drawerFrame(wide, width, 46)];
    for (const frame of frames) for (const line of frame.lines) {
      expect(visibleWidth(line)).toBe(width);
      const text = stripTerminalSequences(line);
      expect(text).not.toContain("Ctrl");
      expect(text).not.toContain("F6");
      // Truncation ends with a full reset; the row background must follow every one of them.
      for (const rest of line.slice(0, -RESET.length).split(RESET).slice(1)) {
        expect(rest.startsWith(PANEL_BG) || rest.startsWith(PANEL_SELECTED)).toBeTrue();
      }
    }
  }
});

test("keyboard ↓ and a pager › click move the same item, and the 80x14 drawer has two rows plus the pager", () => {
  expect(drawerRows(14)).toBe(2);
  const s = source({ files: { scope: "corral", state: "ok", list: Array.from({ length: 12 }, (_, i) => file(`src/f${i}.ts`)) } });
  const keyed = new Sidebar(() => {}), clicked = new Sidebar(() => {});
  for (const bar of [keyed, clicked]) { bar.tab = "files"; bar.drawer = true; bar.focused = true; }
  const first = clicked.drawerFrame(s, 80, 14);
  expect(first.lines).toHaveLength(3);
  expect(plain(first)[0]).toContain("1/12");
  const next = (first.hits.get(0) ?? []).find(hit => hit.page === 1);
  expect(next).toBeDefined();
  if (next) expect(clicked.click(next, s)).toBeUndefined();
  keyed.drawerFrame(s, 80, 14);
  expect(keyed.handleKey("\x1b[B", s).handled).toBeTrue();
  const byClick = plain(clicked.drawerFrame(s, 80, 14)), byKey = plain(keyed.drawerFrame(s, 80, 14));
  expect(byClick).toEqual(byKey);
  expect(byClick[0]).toContain("2–3/12");
  expect(clicked.handleKey("\r", s).open).toEqual({ kind: "file", path: "src/f1.ts" });
  expect(keyed.handleKey("\r", s).open).toEqual({ kind: "file", path: "src/f1.ts" });
});

test("an untracked or binary file shows new or bin, never +0 −0", () => {
  const s = source({ files: { scope: "corral", state: "ok", list: [file("new.ts", "?", null, null), file("logo.png", "M", null, null), file("a.ts", "M", 3, 1)] } });
  const bar = new Sidebar(() => {});
  bar.tab = "files";
  const lines = plain(bar.rail(s, 34, 20)).map(line => line.trimEnd());
  expect(lines.find(line => line.includes("new.ts"))).toMatch(/ new$/);
  expect(lines.find(line => line.includes("logo.png"))).toMatch(/ bin$/);
  expect(lines.find(line => line.includes("a.ts"))).toMatch(/\+3 −1$/);
  expect(lines.join("\n")).not.toContain("+0");
  // The total counts only files with line counts.
  expect(lines.some(line => line.includes("3 files") && line.endsWith("+3 −1"))).toBeTrue();
});
