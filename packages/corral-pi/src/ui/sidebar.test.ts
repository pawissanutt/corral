import { test, expect } from "bun:test";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import {
  Sidebar, drawerRows, type SideAgent, type SideFile, type SideFrame, type SideJob, type SideSource, type SideTab,
} from "./sidebar.ts";
import { PANEL_BG, PANEL_BLUE, PANEL_SELECTED, RESET } from "./panels-view.ts";
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
const SPIN = /[⠋⠙⠹⠸⠼⠴⠦⠧]/;
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
  expect(short.find(line => new RegExp(SPIN.source + " build").test(line))).toBeDefined();
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
      // The one key cap is Ctrl+B beside the toggle.
      if (text.includes("Ctrl")) expect(text.trimEnd()).toMatch(/Ctrl\+B +[▸▴▾]$/);
      expect(text).not.toContain("F6");
      expect(text).not.toContain("unavailable");
      expect(text).not.toContain("unbound");
      // Truncation ends with a full reset; the row background must follow every one of them.
      for (const rest of line.slice(0, -RESET.length).split(RESET).slice(1)) {
        expect(rest.startsWith(PANEL_BG) || rest.startsWith(PANEL_SELECTED)).toBeTrue();
      }
    }
  }
});

test("PageDown and a pager › click turn to the same page, ↓ scrolls by the least, and the 80x14 drawer has two rows plus the pager", () => {
  expect(drawerRows(14)).toBe(2);
  const s = source({ files: { scope: "corral", state: "ok", list: Array.from({ length: 12 }, (_, i) => file(`src/f${i}.ts`)) } });
  const keyed = new Sidebar(() => {}), clicked = new Sidebar(() => {}), stepped = new Sidebar(() => {});
  for (const bar of [keyed, clicked, stepped]) { bar.tab = "files"; bar.drawer = true; bar.focused = true; }
  // Below 16 terminal rows the rule goes, so line 0 is the tab row with the pager: items 1 and 2 of 12.
  const first = clicked.drawerFrame(s, 80, 14);
  expect(first.lines).toHaveLength(3);
  expect(plain(first)[0]!.trimEnd()).toMatch(/^ Context .*‹ 1–2\/12 ›  Ctrl\+B  ▾$/);
  // From 16 rows the rule is line 0 and the tab row with its pager moves to line 1.
  const tall = new Sidebar(() => {});
  tall.tab = "files";
  const ruled = plain(tall.drawerFrame(s, 80, 16));
  expect(ruled[0]).toBe("─".repeat(80)); expect(ruled[1]).toContain("1–4/12");
  const next = (first.hits.get(0) ?? []).find(hit => hit.page === 1);
  expect(next).toBeDefined();
  if (next) expect(clicked.click(next, s)).toBeUndefined();
  keyed.drawerFrame(s, 80, 14);
  expect(keyed.handleKey("\x1b[6~", s).handled).toBeTrue();
  const byClick = plain(clicked.drawerFrame(s, 80, 14)), byKey = plain(keyed.drawerFrame(s, 80, 14));
  expect(byClick).toEqual(byKey);
  expect(byClick[0]).toContain("3–4/12");
  expect(clicked.handleKey("\r", s).open).toEqual({ kind: "file", path: "src/f2.ts" });
  expect(keyed.handleKey("\r", s).open).toEqual({ kind: "file", path: "src/f2.ts" });
  // ↓ past the page's last item moves the window by one item, not a page.
  stepped.drawerFrame(s, 80, 14);
  for (let i = 0; i < 2; i++) expect(stepped.handleKey("\x1b[B", s).handled).toBeTrue();
  expect(plain(stepped.drawerFrame(s, 80, 14))[0]).toContain("2–3/12");
  expect(stepped.handleKey("\r", s).open).toEqual({ kind: "file", path: "src/f2.ts" });
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
  // No totals row: the rows are the files.
  expect(lines.some(line => line.includes("3 files"))).toBeFalse();
});

test("the rail starts with Context Files Plan and [Ctrl+B] ▸, the agents directly below, each name bold in its hue", () => {
  const s = source({
    agents: [agent("c:host-slots", "host-slots", { profile: "deep", activity: "grep \"atomic_write\" crates/host/src/slots.rs and more" }),
      agent("c:tests", "tests", { profile: "quick", glyph: "○", tone: "muted", activity: "waits for host-slots" }),
      agent("c:explore", "explore", { glyph: "✓", tone: "success", settled: "ok", activity: "" })],
    jobs: null, bound: false,
  });
  const bar = new Sidebar(() => {}), frame = bar.rail(s, 34, 20), lines = plain(frame);
  expect(lines[0]!.trimStart().startsWith("Context Files Plan")).toBeTrue();
  expect(lines[0]!.trimEnd()).toMatch(/Ctrl\+B  ▸$/);
  // The whole `[Ctrl+B] ▸` toggles; the tab labels switch tabs.
  const toggle = (frame.hits.get(0) ?? []).filter(hit => hit.toggle);
  expect(Math.min(...toggle.map(hit => hit.from))).toBe(lines[0]!.indexOf("Ctrl") - 1);
  expect((frame.hits.get(0) ?? []).filter(hit => hit.tab).map(hit => hit.tab)).toEqual(["context", "files", "plan"]);
  expect(lines[1]!.trim()).toBe("AGENTS");
  expect(lines[2]).toMatch(new RegExp("^ " + SPIN.source + " host-slots deep"));
  expect(lines[3]!.startsWith("   grep")).toBeTrue(); expect(lines[3]!.trimEnd().endsWith("…")).toBeTrue();
  expect(lines[4]!.startsWith(" ○ tests quick")).toBeTrue();
  expect(frame.lines[2]).toContain(HUE + "\x1b[1mhost-slots");
  expect(lines.indexOf(" COMPLETED · 1".padEnd(34))).toBeGreaterThan(5);
  expect(lines.some(line => line.trim() === "explore")).toBeTrue();
  const text = lines.join("\n");
  expect(text).not.toContain("JOBS"); expect(text).not.toContain("Jobs");
  // Nothing in the rail fills a background but the selected row.
  for (const line of frame.lines) expect(line).not.toContain(PANEL_SELECTED);
});

test("the agent the Agents view selects is drawn on the selected surface with the accent bar, both of its rows", () => {
  const s = source({ agents: [agent("c:a", "alpha"), agent("c:b", "beta")], current: "c:b" });
  const frame = new Sidebar(() => {}).rail(s, 34, 12);
  const rows = frame.lines.flatMap((line, at) => line.includes(PANEL_SELECTED) ? [at] : []), lines = plain(frame);
  expect(rows).toHaveLength(2);
  expect(lines[rows[0]!]).toMatch(new RegExp("^▌" + SPIN.source + " beta"));
  expect(lines[rows[1]!]!.trim()).toBe("read src/a.ts");
  expect(frame.lines[rows[0]!]).toContain(PANEL_BLUE + "▌");
});

test("jobs show only while one is listed, and a session with neither agents nor jobs says so in the mock's words", () => {
  const empty = plain(new Sidebar(() => {}).rail(source({ agents: null, jobs: null }), 34, 10)).join("\n");
  expect(empty).toContain("no agents or jobs yet");
  const jobsOnly = plain(new Sidebar(() => {}).rail(source({ agents: null, jobs: [job("j:1", "bun test")] }), 34, 10));
  expect(jobsOnly.join("\n")).not.toContain("AGENTS");
  expect(jobsOnly[1]!.trim()).toBe("JOBS");
  expect(jobsOnly[2]!.trimEnd()).toMatch(new RegExp("^ " + SPIN.source + " bun test +12s$"));
  const idle = plain(new Sidebar(() => {}).rail(source({ agents: [agent("c:a", "alpha", { settled: "ok", glyph: "✓", tone: "success" })], jobs: [] }), 34, 10));
  expect(idle.some(line => line.trim() === "none at work")).toBeTrue();
});

test("below the rail: a rule, then the collapsed row with summaries and [Ctrl+B] ▴; tiny keeps one count and drops the cap", () => {
  const progress = { revision: 1, spec_revision: "s", phases: [{ id: "p", label: "Build", steps: [
    step("a", "done"), ...["b", "c", "d", "e", "f", "g"].map(id => step(id, "pending"))] }] } as TodoProgress;
  const s = source({
    agents: [agent("c:1", "one"), agent("c:2", "two"), agent("c:3", "three"), agent("c:4", "four", { glyph: "○", tone: "muted" })],
    files: { scope: "corral", state: "ok", list: [file("a.ts")] }, plan: { progress, stale: false },
  });
  const bar = new Sidebar(() => {});
  const narrow = bar.row(s, 92, false, 40), lines = plain(narrow);
  expect(lines).toHaveLength(2);
  expect(lines[0]).toBe("─".repeat(92));
  expect(lines[1]!.trimStart().startsWith("Context 3 running · 1 waiting  Files  Plan 1/7")).toBeTrue();
  expect(lines[1]!.trimEnd()).toMatch(/Ctrl\+B  ▴$/);
  // Hits move down with their row: a tab label opens the drawer on that tab.
  const plan = (narrow.hits.get(1) ?? []).find(hit => hit.tab === "plan")!;
  expect(lines[1]!.slice(plan.from, plan.to)).toBe("Plan 1/7");
  bar.click(plan, s);
  expect(bar.drawer).toBeTrue(); expect(bar.tab).toBe("plan");
  const tiny = plain(new Sidebar(() => {}).row(s, 46, true, 40));
  expect(tiny[1]!.trimStart().startsWith("Context 3  Files  Plan 1/7")).toBeTrue();
  expect(tiny[1]).not.toContain("Ctrl"); expect(tiny[1]!.trimEnd().endsWith("▴")).toBeTrue();
  // Under 16 rows the rule goes first.
  expect(new Sidebar(() => {}).row(s, 92, false, 14).lines).toHaveLength(1);
  const drawer = plain(bar.drawerFrame(s, 92, 40));
  expect(drawer[0]).toBe("─".repeat(92));
  expect(drawer[1]!.trimEnd()).toMatch(/Ctrl\+B  ▾$/);
  expect(drawer[2]!.trim()).toBe("1/7 done");
});
