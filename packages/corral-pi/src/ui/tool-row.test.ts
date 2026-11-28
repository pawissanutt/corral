import { expect, test } from "bun:test";
import type { ToolRenderers } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";

import type { Ticker } from "./activity.ts";
import { baseState, clipMiddle, rowBlock, rowLine, rowRenderers, shortPath, stringArg } from "./tool-row.ts";
import type { Presenter, RowModel } from "./tool-row.ts";

type Ctx = Parameters<NonNullable<ToolRenderers["renderCall"]>>[2];
const SPINNING = /^[⠋⠙⠹⠸⠼⠴⠦⠧●]$/u;
let now = 1_000_000;
const holds: string[] = [], released: string[] = [];
const ticker: Ticker = { now: () => now, hold: id => { holds.push(id); return () => { released.push(id); }; } };
const deps = { ticker: () => ticker };

function context(args: Record<string, unknown>, over: Partial<Ctx> = {}): Ctx {
  return { args, toolCallId: "call-1", invalidate() {}, lastComponent: undefined, state: {}, cwd: "/work", executionStarted: true,
    argsComplete: true, isPartial: true, expanded: false, showImages: false, isError: false, durationMs: undefined, outputPad: 1, ...over } as Ctx;
}
function draw(present: Presenter, ctx: Ctx, width = 100, result?: { text: string; isError?: boolean; partial?: boolean }): string[] {
  const renderers = rowRenderers(present, deps, "probe");
  const row = renderers.renderCall!(ctx.args, undefined as never, ctx);
  if (result) renderers.renderResult!({ content: [{ type: "text", text: result.text }], details: undefined },
    { expanded: ctx.expanded, isPartial: result.partial ?? false }, undefined as never, { ...ctx, isError: result.isError ?? false });
  return row.render(width).map(line => stripTerminalSequences(line));
}
const probe: Presenter = call => {
  const state = baseState(call);
  return { state, verb: "probe", target: stringArg(call.args, "command") ?? "",
    ...(state === "failed" ? { body: [{ text: "line one" }, { text: "line two" }], hiddenHint: "12 more lines" } : {}) };
};

test("arguments still streaming read writing with the part written so far", () => {
  const [line] = draw(probe, context({ command: "cargo te" }, { argsComplete: false, executionStarted: false }));
  expect(line).toContain("writing");
  expect(line).toContain("cargo te");
  expect(line!.trim()[0]).toMatch(SPINNING);
});

test("a running call shows the spinner and the elapsed time since it started", () => {
  const ctx = context({ command: "cargo test" });
  const renderers = rowRenderers(probe, deps, "probe");
  const row = renderers.renderCall!(ctx.args, undefined as never, ctx);
  row.render(100);
  now += 12_000;
  const line = stripTerminalSequences(row.render(100)[0]!);
  expect(line.trim()[0]).toMatch(SPINNING);
  expect(line).toContain("12s");
});

test("a failure differs from a success in glyph and words, without colour", () => {
  const ok = draw(probe, context({ command: "true" }, { isPartial: false }), 100, { text: "fine" });
  const failed = draw(probe, context({ command: "false" }, { isPartial: false }), 100, { text: "boom", isError: true });
  expect(ok[0]).toContain("✓");
  expect(failed[0]).toContain("✗");
  expect(failed[0]).toContain("failed");
  expect(failed.slice(1).join("\n")).toContain("line two");
  expect(failed.at(-1)).toContain("12 more lines");
});

test("at 46 columns the row stays one line with glyph, verb and time", () => {
  const model: RowModel = { state: "ok", verb: "bash", target: "cargo test -p corral-facts --lib runtime_cargo_workspace_golden",
    outcome: "exit 0 · test result: ok. 14 passed; 0 failed", right: "j14-439", duration: 225_000 };
  for (const width of [185, 100, 92, 61, 46]) {
    const { lines } = rowBlock(model, width, now, false);
    const line = stripTerminalSequences(lines[0]!);
    expect(lines).toHaveLength(1);
    expect(visibleWidth(line)).toBeLessThanOrEqual(width);
    expect(line.startsWith("✓ bash")).toBe(true);
    expect(line).toContain("3m 45s");
  }
});

test("at 46 columns a short outcome stays whole, a long one never pushes the target under 12 cells", () => {
  const plain = (model: RowModel) => stripTerminalSequences(rowLine(model, 46, now));
  const short = plain({ state: "ok", verb: "diagnostics", target: "crates/ops/src/attention/review_tests.rs", targetClip: "middle", outcome: "no diagnostics", duration: 2300 });
  expect(short).toContain("no diagnostics");
  expect(short).toContain("2.3s");
  const long = plain({ state: "waiting", verb: "bash", target: "cargo check -p corral-engine >.evidence/c3-check.log", outcome: "continued in the background", duration: 1100 });
  expect(long).toContain("cargo check");
  const failed = plain({ state: "failed", verb: "ls", target: "~/miti-workspace/corral/.worktrees/T-854/crates", targetClip: "middle", outcome: "Path not found: /home/pawis/miti-workspace/corral/.worktrees/T-854/crates" });
  expect(failed).toContain("crates");
  for (const line of [short, long, failed]) expect(visibleWidth(line)).toBeLessThanOrEqual(46);
  expect(clipMiddle("T-949/R3 88153bf1-T-949-R3-3767185-1794368790486565526.log", 25)).toMatch(/^T-949\/R3….*\.log$/);
});

test("paths read relative to the session and clip in the middle so the file name stays", () => {
  expect(shortPath("/work/src/ui/tool-row.ts", "/work")).toBe("src/ui/tool-row.ts");
  const clipped = clipMiddle("packages/corral-pi/src/ui/build/tool-renderers.ts", 30);
  expect(visibleWidth(clipped)).toBe(30);
  expect(clipped.endsWith("tool-renderers.ts")).toBe(true);
});

test("a click on the view label opens it; other clicks fall through to Pi's expand toggle", () => {
  let opened = 0;
  const present: Presenter = () => ({ state: "ok", verb: "bash", view: { label: "Agents", open: () => { opened++; } } });
  const ctx = context({}, { isPartial: false });
  const renderers = rowRenderers(present, deps, "probe");
  const row = renderers.renderCall!(ctx.args, undefined as never, ctx);
  const line = stripTerminalSequences(row.render(80)[0]!);
  const click = { type: "click" as const, button: "left" as const, y: 0, screenX: 0, screenY: 0, width: 80, height: 1, shift: false, alt: false, ctrl: false };
  expect(row.handleMouse?.({ ...click, x: 3 })).toBeUndefined();
  expect(row.handleMouse?.({ ...click, x: line.lastIndexOf("Agents") })?.handled).toBe(true);
  expect(opened).toBe(1);
});

test("a live row holds the ticker until it settles", () => {
  let live = true;
  const present: Presenter = () => live ? { state: "running", verb: "bash", live: "row:x" } : { state: "ok", verb: "bash" };
  const ctx = context({}, { isPartial: false });
  const row = rowRenderers(present, deps, "probe").renderCall!(ctx.args, undefined as never, ctx);
  row.render(80); row.render(80);
  expect(holds.filter(id => id === "row:x")).toHaveLength(1);
  live = false;
  row.render(80);
  expect(released).toContain("row:x");
});

test("bounded wrap keeps the last rows of streaming prose", () => {
  const text = Array.from({ length: 40 }, (_, index) => `word${index}`).join(" ");
  const { lines } = rowBlock({ state: "writing", verb: "ask", body: [{ text, wrap: { max: 2, from: "end" } }] }, 40, now, false);
  expect(lines).toHaveLength(3);
  expect(stripTerminalSequences(lines[1]!).trim().startsWith("…")).toBe(true);
  expect(stripTerminalSequences(lines[2]!)).toContain("word39");
});
