import { expect, test } from "bun:test";
import type { ToolRenderers } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import type { CellReport, ChildOutcome } from "../extensions/codemode/report.ts";
import type { Ticker } from "./activity.ts";
import { type CellSource, type CodeCellDeps, cellRenderers, foldChildren } from "./code-cell.ts";
import type { CallInput, RowModel } from "./tool-row.ts";

type RenderContext = Parameters<NonNullable<ToolRenderers["renderCall"]>>[2];
const NOW = 1_800_000_000_000;

/** A presenter stand-in: the tool name, its path or command, and the failure reason as the outcome. */
function present(call: CallInput): RowModel {
  const target = typeof call.args.path === "string" ? call.args.path : typeof call.args.command === "string" ? call.args.command : undefined;
  const text = call.result?.content.flatMap(part => part.type === "text" ? [part.text] : []).join("\n") ?? "";
  return { state: call.isError ? "failed" : call.result ? "ok" : "running", verb: call.name, ...(target ? { target } : {}),
    ...(call.isError ? { outcome: text, outcomeTone: "error" as const } : {}) };
}
function deps(source?: CellSource, ticker?: Ticker): CodeCellDeps {
  return { ticker: () => ticker, code: () => source, presenter: () => present };
}
function child(n: number, tool: string, ok: boolean | undefined, extra: Partial<ChildOutcome> = {}): ChildOutcome {
  return { tool, call: `cell-1.${n}`, key: `t/cell-1.${n}`, startedAt: NOW, ...(ok === undefined ? {} : { ok, durationMs: 10 + n }), ...extra };
}
function report(children: readonly ChildOutcome[], extra: Partial<CellReport> = {}): CellReport {
  const settledOk = children.filter(c => c.ok === true).length, failed = children.filter(c => c.ok === false).length;
  return { cell: "cell-1", status: "ok", stateLost: false, truncated: false, outputs: [], children, startedAt: NOW,
    calls: { total: children.length, ok: settledOk, failed, running: children.length - settledOk - failed, committed: 0 },
    failures: children.filter(c => c.ok === false), durationMs: 374, ...extra };
}
function context(args: Record<string, unknown>, extra: Partial<RenderContext> = {}): RenderContext {
  return { args, toolCallId: "t", invalidate: () => {}, lastComponent: undefined, state: {}, cwd: "/repo", executionStarted: true,
    argsComplete: true, isPartial: false, expanded: false, showImages: false, isError: false, outputPad: 1, ...extra } as RenderContext;
}
function draw(renderers: ToolRenderers, ctx: RenderContext, cells: readonly CellReport[] | undefined, width = 100, partial = false): string[] {
  const component = renderers.renderCall!(ctx.args, undefined as never, ctx);
  if (cells) renderers.renderResult!({ content: [{ type: "text", text: "model text" }], details: { cells } }, { expanded: ctx.expanded, isPartial: partial }, undefined as never, ctx);
  return component.render(width).map(line => stripTerminalSequences(line));
}
const code = { code: "await tools.read({path:'a'})" };

test("a cell lists the calls it made, each through its tool's presenter, and folds above five", () => {
  const children = Array.from({ length: 12 }, (_, i) => child(i + 1, "read", true, { args: JSON.stringify({ path: `f${i + 1}.ts` }) }));
  const lines = draw(cellRenderers(deps()), context(code), [report(children)]);
  expect(lines[0]).toContain("12 calls");
  expect(lines[0]).toContain("read ×12");
  expect(lines.filter(line => line.includes("f1.ts") || line.includes("f2.ts") || line.includes("f3.ts") || line.includes("f12.ts"))).toHaveLength(4);
  expect(lines.some(line => line.includes("8 more ✓"))).toBeTrue();
  expect(lines.some(line => line.includes("f7.ts"))).toBeFalse();
  for (const width of [46, 61, 92, 100, 185]) {
    for (const line of draw(cellRenderers(deps()), context(code), [report(children)], width)) expect(visibleWidth(line)).toBeLessThanOrEqual(width);
  }
});

test("a failed child below the fold stays visible, and a cell whose script caught it reads ok with the failure counted", () => {
  const children = [...Array.from({ length: 9 }, (_, i) => child(i + 1, "read", true)),
    child(10, "read", false, { args: JSON.stringify({ path: "missing.md" }), code: "tool_error", message: "ENOENT: no such file" }),
    child(11, "bash", true)];
  expect(foldChildren(children).some(entry => "child" in entry && entry.child === children[9])).toBeTrue();
  const lines = draw(cellRenderers(deps()), context(code), [report(children)]);
  expect(lines[0]).toMatch(/^\s*✓ code/);
  expect(lines[0]).toContain("✗1");
  expect(lines.some(line => line.includes("missing.md") && line.includes("ENOENT"))).toBeTrue();
});

test("running children are capped at three and the rest fold into one line", () => {
  const children = [...Array.from({ length: 11 }, (_, i) => child(i + 1, "read", true)),
    ...Array.from({ length: 9 }, (_, i) => child(i + 12, "read", undefined))];
  // A foreground update of a call that is still executing: the only stored report that counts as live.
  const lines = draw(cellRenderers(deps()), context(code, { isPartial: true }),
    [report(children, { status: "running", durationMs: undefined })], 100, true);
  expect(lines.filter(line => /^\s+[⠋⠙⠹⠸⠼⠴⠦⠧●] read/.test(line))).toHaveLength(3);
  expect(lines.some(line => line.includes("6 more running"))).toBeTrue();
  expect(lines.some(line => line.includes("7 more ✓"))).toBeTrue();
});

test("printed JSON is one clipped row, and no row names a key", () => {
  const printed = JSON.stringify({ bytes: 245831, next_offset: 4096, offset: 0, reference: "artifacts/x.log" }, null, 2);
  const lines = draw(cellRenderers(deps()), context(code), [report([child(1, "read", true)], {
    outputs: [{ type: "text", text: printed }, { type: "text", text: "a\nb\nc\nd\ne\nf\ng" }] })]);
  expect(lines.filter(line => line.includes("bytes"))).toHaveLength(1);
  expect(lines.some(line => line.includes("more rows"))).toBeTrue();
  expect(lines.join("\n")).not.toMatch(/Ctrl\+O|click/);
});

test("a detached cell keeps redrawing from the runtime until it settles, then releases its watch and the clock", () => {
  let current: CellReport = report([child(1, "read", true), child(2, "bash", undefined)], { status: "detached", durationMs: undefined });
  let changed: (() => void) | undefined, released = 0;
  const holds: string[] = [];
  const source: CellSource = { snapshot: () => current, watch: (_id, fn) => { changed = fn; return () => { released++; }; } };
  const ticker: Ticker = { now: () => NOW + 61_000, hold: id => { holds.push(id); return () => { released++; }; } };
  const renderers = cellRenderers(deps(source, ticker));
  const ctx = context(code);
  let redraws = 0;
  ctx.invalidate = () => { redraws++; };
  const first = draw(renderers, ctx, [report(current.children, { status: "detached", durationMs: undefined })]);
  expect(first[0]).toContain("in background");
  expect(holds).toEqual(["code:t"]);
  // The runtime reports settlement; no tool event arrives. Its watch callback is the row's only trigger.
  current = report([child(1, "read", true), child(2, "bash", false, { message: "Command timed out after 60 seconds" })],
    { status: "error", error: { name: "CodeCallError", message: "Command timed out after 60 seconds" }, durationMs: 60_000 });
  changed?.();
  expect(redraws).toBe(1);
  const after = renderers.renderCall!(ctx.args, undefined as never, ctx).render(100).map(line => stripTerminalSequences(line));
  expect(after[0]).toMatch(/^\s*✗ code/);
  expect(after[0]).toContain("settled in background");
  expect(after.some(line => line.includes("timed out"))).toBeTrue();
  expect(released).toBe(2);
});

test("a stored detached report the runtime does not know is history: no spinner, no clock, no watch", () => {
  const holds: string[] = [];
  let watched = 0;
  const source: CellSource = { snapshot: () => undefined, watch: () => { watched++; return () => {}; } };
  const ticker: Ticker = { now: () => NOW, hold: id => { holds.push(id); return () => {}; } };
  const stored = report([child(1, "read", undefined), child(2, "bash", undefined)], { status: "detached", durationMs: undefined });
  const lines = draw(cellRenderers(deps(source, ticker)), context(code), [stored]);
  expect(lines.join("\n")).not.toMatch(/[⠋⠙⠹⠸⠼⠴⠦⠧]/);
  expect(lines[0]).toMatch(/^\s*○ code/);
  expect(lines[0]).toContain("moved to the background");
  expect(lines.filter(line => /^\s+! /.test(line) && line.includes("outcome not recorded"))).toHaveLength(2);
  expect(holds).toEqual([]);
  expect(watched).toBe(0);
});

test("a report stored before child records existed renders its counts and failures", () => {
  const old = { cell: "cell-6", status: "error", generation: 1, stateLost: false, truncated: false, outputs: [],
    calls: { total: 4, ok: 0, failed: 1, running: 3, committed: 0 }, error: { name: "CodeCallError", code: "tool_error", message: "ENOENT" },
    failures: [{ tool: "read", call: "cell-6.2", key: "x/cell-6.2", ok: false, code: "tool_error", message: "ENOENT", effect: "unknown" }] };
  let lines: string[] = [];
  expect(() => { lines = draw(cellRenderers(deps()), context(code), [old as unknown as CellReport]); }).not.toThrow();
  expect(lines[0]).toMatch(/^\s*✗ code/);
  expect(lines[0]).toContain("4 calls");
  expect(lines.some(line => line.includes("ENOENT"))).toBeTrue();
});

test("a script still streaming shows its newest lines, not escaped JSON", () => {
  const script = Array.from({ length: 8 }, (_, i) => `const v${i} = await tools.read({ path: "f${i}" });`).join("\n");
  const lines = draw(cellRenderers(deps()), context({ code: script }, { argsComplete: false, executionStarted: false }), undefined);
  expect(lines[0]).toContain("8 lines");
  expect(lines.some(line => line.includes("const v7"))).toBeTrue();
  expect(lines.some(line => line.includes("const v2"))).toBeFalse();
  expect(lines.join("\n")).not.toContain("\\n");
});

test("a cancel names its cell and says it had already finished", () => {
  const notFound: CellReport = { ...report([]), cell: "cell-2", status: "not_found" };
  const lines = draw(cellRenderers(deps()), context({ cancel: "cell-2" }), [notFound]);
  expect(lines[0]).toContain("cell 2");
  expect(lines[0]).toContain("already finished");
});
