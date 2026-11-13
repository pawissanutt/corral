import { beforeAll, expect, test } from "bun:test";
import { initTheme } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";

import { filePresenters } from "./tool-lines-files.ts";
import type { ToolRenderDeps } from "./tool-rendering.ts";
import { rowLine, type CallInput, type Presenter, type RowModel } from "./tool-row.ts";

beforeAll(() => initTheme("dark", false));

const cwd = "/repo";
const deps: ToolRenderDeps = { cwd, ticker: () => undefined, job: () => undefined, code: () => undefined, ledger: () => undefined, corralSpecs: [] };
function present(name: string): Presenter {
  const presenter = filePresenters.presenter(name, deps);
  if (!presenter) throw new Error(`no presenter for ${name}`);
  return presenter;
}
function call(name: string, args: Record<string, unknown>, more: Partial<CallInput> = {}): CallInput {
  return { name, args, argsComplete: true, executionStarted: true, partial: false, isError: false, expanded: false, cwd, toolCallId: "c1", ...more };
}
function done(text: string, details?: unknown, isError = false): Partial<CallInput> {
  return { result: { content: [{ type: "text", text }], details }, isError };
}
function expandedText(model: RowModel): string {
  const view = model.expanded?.(100);
  if (!view) return "";
  return stripTerminalSequences(("render" in view ? view.render(100) : view.map(line => line.text)).join("\n"));
}

test("a write whose content streams shows its newest lines, in a body that grows only once", () => {
  const content = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
  const streaming = { argsComplete: false, executionStarted: false };
  const early = present("write")(call("write", { path: "src/a.ts", content: content.slice(0, 40) }, streaming));
  const later = present("write")(call("write", { path: "src/a.ts", content }, streaming));
  expect(early.state).toBe("writing");
  expect(early.body?.length).toBe(later.body?.length);
  expect(later.body?.at(-1)?.text).toBe("line 30");
  expect(early.body?.some(line => line.text === "line 30")).toBeFalse();
  const finished = present("write")(call("write", { path: "src/a.ts", content }, done("Successfully wrote to src/a.ts")));
  expect(finished.state).toBe("ok");
  expect(finished.body).toBeUndefined();
  expect(finished.outcome).toContain("30 lines");
});

test("an edit with two changes is one row with its diffstat, and expands to the SDK's diff", () => {
  const patch = "--- a.ts\n+++ a.ts\n@@ -1,2 +1,2 @@\n-old one\n+new one\n@@ -9,1 +9,1 @@\n-old two\n+new two";
  const diff = "-1 old one\n+1 new one\n   ...\n-9 old two\n+9 new two";
  const model = present("edit")(call("edit", { path: "/repo/src/a.ts", edits: [{ oldText: "old one", newText: "new one" }, { oldText: "old two", newText: "new two" }] },
    done("Successfully replaced 2 blocks", { diff, patch })));
  expect(model.state).toBe("ok");
  expect(model.target).toBe("src/a.ts");
  expect(model.outcome).toBe("+2 \u22122 in 2 hunks");
  expect(model.body).toBeUndefined();
  expect(expandedText(model)).toContain("new two");
});

test("an edit being written shows its replacement as it grows", () => {
  const model = present("edit")(call("edit", { path: "src/a.ts", edits: [{ oldText: "a\nb", newText: "c\nd\ne" }] }, { argsComplete: false, executionStarted: false }));
  expect(model.state).toBe("writing");
  expect(model.body?.filter(line => line.tone === "added").map(line => line.text)).toEqual(["+ c", "+ d", "+ e"]);
  expect(model.body?.filter(line => line.tone === "removed").length).toBe(2);
});

test("a grep that hit its limit says so; one that found nothing is an ok outcome", () => {
  const output = "a.rs:1: one\na.rs-2- context\nb.rs:3: two\n\n[35 matches limit reached. Use limit=70 for more, or refine pattern]";
  const limited = present("grep")(call("grep", { pattern: "one", path: "src" }, done(output, { matchLimitReached: 35 })));
  expect(limited.outcome).toContain("35+ matches");
  expect(limited.outcome).toContain("2 files");
  const none = present("grep")(call("grep", { pattern: "zzz" }, done("No matches found")));
  expect(none.state).toBe("ok");
  expect(none.outcome).toBe("no matches");
});

test("a failed read shows the failure and its reason", () => {
  const model = present("read")(call("read", { path: "missing.ts" },
    done("ENOENT: no such file or directory, access '/repo/missing.ts'", undefined, true)));
  expect(model.state).toBe("failed");
  expect(model.outcome).toBe("not found");
  expect(model.body?.[0]?.text).toContain("ENOENT");
});

test("a read reports how much of the file it showed", () => {
  const model = present("read")(call("read", { path: "src/a.ts", offset: 10, limit: 3 }, done("x\ny\nz\n\n[7 more lines in file. Use offset=13 to continue.]")));
  expect(model.target).toBe("src/a.ts:10-12");
  expect(model.outcome).toBe("3 lines · 7 more");
});

test("a clipped result counts from its recorded totals, never from the preview", () => {
  const preview = done("first\nsecond\nthird");
  expect(present("read")(call("read", { path: "src/a.ts" }, { ...preview, totals: { lines: 120 } })).outcome).toBe("120 lines");
  expect(present("read")(call("read", { path: "src/a.ts" }, { ...preview, totals: {} })).outcome).toBeUndefined();
  expect(present("ls")(call("ls", { path: "src" }, { ...preview, totals: {} })).outcome).toBeUndefined();
});

test("at 46 columns a collapsed file row is one line and keeps the file name", () => {
  const long = "/repo/.corral/artifacts/T-762/D1/af9a77f3-plan.md/af9a77f3-plan.md";
  const rows = [
    present("read")(call("read", { path: long, offset: 1, limit: 450 }, done("ENOTDIR: not a directory, open", undefined, true))),
    present("write")(call("write", { path: long, content: "a\nb" }, done("Successfully wrote"))),
    present("edit")(call("edit", { path: long, edits: [] }, done("ok", { diff: "-1 a\n+1 b", patch: "@@ -1 +1 @@\n-a\n+b" }))),
  ];
  for (const model of rows) {
    const line = rowLine(model, 46, 0);
    expect(line.includes("\n")).toBeFalse();
    expect(visibleWidth(line)).toBeLessThanOrEqual(46);
    expect(stripTerminalSequences(line)).toContain("plan.md");
  }
});
