import { expect, test } from "bun:test";
import type { AgentToolResult, ToolRenderers } from "@earendil-works/pi-coding-agent";
import { stripTerminalSequences } from "@earendil-works/pi-tui";

import { presenterFor, type ToolRenderDeps } from "./tool-rendering.ts";
import { LOCAL_TOOL_NAMES, localPresenters } from "./tool-lines-local.ts";
import { rowRenderers } from "./tool-row.ts";

type Ctx = Parameters<NonNullable<ToolRenderers["renderCall"]>>[2];
const deps: ToolRenderDeps = { cwd: "/work", ticker: () => undefined, job: () => undefined, code: () => undefined, ledger: () => undefined, corralSpecs: [] };
const ARGS = { path: "src/a.ts", line: 3, column: 5, url: "https://example.dev/docs", query: "save slots", content_id: "9fcb9516-1b56",
  questions: [{ header: "Delivery check", question: "Send it now?", options: ["yes", "no"] }], question: "Ship it?", prompt: "Trace the slot store",
  server: "github", uri: "repo://x", tools: ["create_issue"], document: "plan", offset: 0, title: "x" };

function row(name: string, args: Record<string, unknown>, result?: { result: AgentToolResult<unknown>; isError?: boolean; partial?: boolean }, label?: string) {
  const ctx = { args, toolCallId: `${name}-1`, invalidate() {}, lastComponent: undefined, state: {}, cwd: "/work", executionStarted: true,
    argsComplete: true, isPartial: result === undefined || result.partial === true, expanded: false, showImages: false,
    isError: result?.isError ?? false, durationMs: undefined, outputPad: 1 } as Ctx;
  const renderers = rowRenderers(presenterFor(name, deps, label), deps, name);
  const component = renderers.renderCall!(args, undefined as never, ctx);
  if (result) renderers.renderResult!(result.result, { expanded: false, isPartial: result.partial ?? false }, undefined as never, ctx);
  return component.render(100).map(line => stripTerminalSequences(line));
}
const reply = (text: string, details?: unknown): AgentToolResult<unknown> => ({ content: [{ type: "text", text }], details });

test("every registered local tool has its own presenter and never prints key=\"json\" arguments", () => {
  for (const name of [...LOCAL_TOOL_NAMES, "mcp_github_create_issue_0a1b2c3d4e"]) {
    expect(localPresenters.presenter(name, deps, "github: Create issue")).toBeDefined();
    const [line] = row(name, ARGS, undefined, "github: Create issue");
    expect(line).not.toMatch(/\w="/);
    expect(line).not.toContain("{");
  }
});

test("a 24 KiB one-line JSON result collapses to one row", () => {
  const body = { url: "https://example.dev/docs", status: 200, total_bytes: 24_576, body: "# Docs\n" + "x".repeat(24_000), next_offset: 24_576 };
  const lines = row("web_fetch", { url: body.url }, { result: reply(JSON.stringify(body)) });
  expect(lines).toHaveLength(1);
  expect(lines[0]).toContain("✓ fetch example.dev/docs");
  expect(lines[0]).toContain("200");
});

test("a diagnostics timeout reads as a failed timeout; a clean file reads no diagnostics", () => {
  const timeout = row("lsp_diagnostics", { path: "src/a.ts" }, { result: reply("lsp_timeout: typescript-language-server is still indexing "
    + "or has not published fresh diagnostics after 120000 ms; retry lsp_diagnostics when ready"), isError: true });
  expect(timeout[0]).toContain("✗");
  expect(timeout[0]).toContain("timeout after 2m");
  const clean = row("lsp_diagnostics", { path: "src/a.ts" }, { result: reply(JSON.stringify({ server: "rust-analyzer", total: 0, diagnostics: [] })) });
  expect(clean[0]).toContain("✓ diagnostics src/a.ts");
  expect(clean[0]).toContain("no diagnostics");
  const errors = row("lsp_diagnostics", { path: "src/a.ts" }, { result: reply(JSON.stringify({ total: 1, diagnostics: [
    { line: 37, column: 13, severity: "error", code: "E0063", message: "missing field `x`" }] })) });
  expect(errors[0]).toContain("1 error");
  expect(errors[1]).toContain("37:13");
});

test("an open question reads as waiting for the owner; an answered one shows the chosen option", () => {
  const open = row("ask_user", { questions: ARGS.questions });
  expect(open[0]).toContain("▲");
  expect(open[0]).toContain("waiting for you");
  const answered = row("ask_user", { questions: ARGS.questions }, { result: reply("Send it now?: yes",
    { status: "answered", answers: [{ question: "Send it now?", option: "yes" }] }) });
  expect(answered[0]).toContain("✓");
  expect(answered[0]).toContain("answered: yes");
  const busy = row("ask_user", { questions: ARGS.questions }, { result: reply("no answer", { status: "no answer", reason: "another owner/permission dialog is open" }) });
  expect(busy[0]).toContain("another dialog was open");
});

test("research shows its progress while running and states why partial research stopped", () => {
  const running = row("explore", { prompt: "Trace the slot store", effort: "quick" }, { result: reply("read app/src/state/slots.ts", { progress: { turns: 3, tools: 7 } }), partial: true });
  expect(running[0]).toContain("turn 3");
  expect(running[1]).toContain("read app/src/state/slots.ts");
  const partial = row("explore", { prompt: "Trace the slot store" }, { result: reply("Research partial; stop: turn_limit.\nRequest: abc; generation: 1.\n\nSlots load in useSlots().",
    { text: "Slots load in useSlots().", outcome: "uncertain", report: { status: "partial", stopReason: "turn_limit" }, request: "abc", generation: 1 }) });
  expect(partial[0]).toContain("!");
  expect(partial[0]).toContain("turn limit");
  expect(partial.join("\n")).not.toContain("Request: abc");
});
