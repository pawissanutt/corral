import { expect, test } from "bun:test";
import type { AgentToolResult, ToolRenderers } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";

import type { Ticker } from "./activity.ts";
import { commandLine } from "./tool-lines-jobs.ts";
import { presenterFor, type JobFacts, type ToolRenderDeps } from "./tool-rendering.ts";
import { rowRenderers } from "./tool-row.ts";

type Ctx = Parameters<NonNullable<ToolRenderers["renderCall"]>>[2];
const NOW = 2_000_000;
const holds: string[] = [], released: string[] = [];
const ticker: Ticker = { now: () => NOW, hold: id => { holds.push(id); return () => { released.push(id); }; } };
const jobs = new Map<string, JobFacts>();
const deps: ToolRenderDeps = {
  cwd: "/work", ticker: () => ticker, job: id => jobs.get(id), code: () => undefined, ledger: () => undefined, corralSpecs: [],
};

function row(name: string, args: Record<string, unknown>, result?: { result: AgentToolResult<unknown>; isError?: boolean }) {
  const ctx = { args, toolCallId: `${name}-1`, invalidate() {}, lastComponent: undefined, state: {}, cwd: "/work", executionStarted: true,
    argsComplete: true, isPartial: result === undefined, expanded: false, showImages: false, isError: result?.isError ?? false,
    durationMs: result ? 1500 : undefined, outputPad: 1 } as Ctx;
  const renderers = rowRenderers(presenterFor(name, deps), deps, name);
  const component = renderers.renderCall!(args, undefined as never, ctx);
  if (result) renderers.renderResult!(result.result, { expanded: false, isPartial: false }, undefined as never, ctx);
  return (width = 100) => component.render(width).map(line => stripTerminalSequences(line));
}
function exited(job: string, exitCode: number, text: string): AgentToolResult<unknown> {
  const details = { job, state: "exited", exit_code: exitCode, signal: null, termination: "exit", text };
  return { content: [{ type: "text", text }], details };
}

test("a failed bash shows its exit code and output tail; a successful one is one row with exit 0", () => {
  const ok = row("bash", { command: "cd /work && cargo fmt --all" }, { result: exited("j1-1", 0, "") })();
  expect(ok).toHaveLength(1);
  expect(ok[0]).toContain("✓ bash cargo fmt --all");
  expect(ok[0]).toContain("exit 0");
  const failed = row("bash", { command: "ruff check scripts" }, { result: exited("j1-2", 1, "a\nb\nc\nd\nFound 1 error."), isError: true })();
  expect(failed[0]).toContain("✗ bash");
  expect(failed[0]).toContain("exit 1");
  expect(failed.join("\n")).toContain("Found 1 error.");
  expect(failed.join("\n")).not.toContain("a\n");
});

test("a blocked call reads as failed with its reason", () => {
  const blocked = row("bash", { command: "rm -rf build" }, {
    result: { content: [{ type: "text", text: "shell disabled in plan mode" }], details: undefined }, isError: true })();
  expect(blocked[0]).toContain("✗");
  expect(blocked[0]).toContain("shell disabled in plan mode");
});

test("a backgrounded bash stays running until its job exits, then shows the exit without a new tool event", () => {
  jobs.set("j1-3", { job: "j1-3", command: "cargo test", state: "running", startedAt: NOW - 72_000, endedAt: undefined, exitCode: null, signal: null, tail: "" });
  const draw = row("bash", { command: "cargo test", wait_ms: 1000 }, {
    result: { content: [{ type: "text", text: "job: j1-3\nstate: running" }], details: { job: "j1-3", state: "running", background: true, text: "" } } });
  const running = draw()[0]!;
  expect(running).not.toContain("✓");
  expect(running).toContain("running in the background");
  expect(running).toContain("1m 12s");
  expect(holds).toContain("tool-row:bash-1");
  jobs.set("j1-3", { ...jobs.get("j1-3")!, state: "exited", exitCode: 0, endedAt: NOW, tail: "test result: ok. 14 passed" });
  const done = draw()[0]!;
  expect(done).toContain("✓");
  expect(done).toContain("exit 0");
  expect(released).toContain("tool-row:bash-1");
});

test("job rows name the command, not only the id", () => {
  jobs.set("j1-4", { job: "j1-4", command: "cargo test -p host slots", state: "exited", startedAt: NOW - 5000, endedAt: NOW, exitCode: 101, signal: null, tail: "" });
  const waited = row("job_wait", { job: "j1-4", wait_ms: 1000 }, { result: exited("j1-4", 101, "panicked"), isError: true })();
  expect(waited[0]).toContain("cargo test -p host slots");
  expect(waited[0]).toContain("✗");
  expect(waited[0]).toContain("exit 101");
  const peeked = row("job_output", { job: "j1-4" }, { result: exited("j1-4", 101, "x\ny") })();
  expect(peeked[0]).toContain("cargo test -p host slots");
});

test("at 46 columns a long command stays one line with glyph, verb and state", () => {
  const lines = row("bash", { command: "cargo test -p corral-facts --lib runtime_cargo_workspace_golden" },
    { result: exited("j1-5", 0, "ok") })(46);
  expect(lines).toHaveLength(1);
  expect(lines[0]!.trim().startsWith("✓ bash")).toBe(true);
  expect(lines[0]).toContain("1.5s");
});

test("a command line drops the session's own cd and summarises a heredoc", () => {
  expect(commandLine("cd /work && bun test", "/work")).toBe("bun test");
  expect(commandLine("cd /elsewhere/x && ls", "/work")).toBe("cd /elsewhere/x && ls");
  expect(commandLine("python3 - <<'PY'\nprint(1)\nprint(2)\nPY\necho done", "/work")).toBe("python3 - <<'PY' … PY (2-line heredoc, +1 more lines)");
});
