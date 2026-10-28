import { expect, test } from "bun:test";

import { type AgentToolResult, Theme } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../bridge/events.ts";
import type { CorralReceipt } from "../tools/corral.ts";
import {
  type Alerts, alertsStatus, bindingStatus, countAlert, headerLines, target, titleText,
} from "./chrome.ts";
import { DRACULA, draculaTheme } from "./dracula.ts";
import { corralRenderers, renderCallText, renderResultText } from "./render.ts";

const theme = {
  fg: (color: string, text: string) => `<${color}>${text}</>`,
  bold: (text: string) => `*${text}*`,
};
const plain = { fg: (_color: string, text: string) => text, bold: (text: string) => text };

function receipt(state: CorralReceipt["state"], result: unknown, extra: Partial<CorralReceipt> = {}): AgentToolResult<unknown> {
  const details: CorralReceipt = { version: 1, request_id: "rq-1", op: "ask", state, result, ...extra };
  return { content: [{ type: "text", text: JSON.stringify({ state, result }) }], details };
}

test("a corral call renders as its short name and the one argument that says what it does", () => {
  expect(renderCallText("corral_ask", { kind: "technical_choice", text: "Use\n v0.3.0?", blocking: true }, plain))
    .toBe("corral ask technical_choice: Use v0.3.0?");
  expect(renderCallText("corral_question_answer", { key: "Q-2", rev: 1, text: "Yes" }, plain))
    .toBe("corral question answer Yes");
  expect(renderCallText("corral_progress", { update: { phases: [], summary: "orient done" } }, plain))
    .toBe("corral progress orient done");
  expect(renderCallText("corral_gate", {}, plain)).toBe("corral gate");
  expect(renderCallText("corral_done", { summary: "x".repeat(200) }, plain).length).toBeLessThanOrEqual(120);
});

test("a receipt renders as one line: its state and the result fields worth reading", () => {
  expect(renderResultText(receipt("committed", { key: "Q-3", rev: 0, tier: "quick" }), false, false, plain))
    .toBe("✓ key=Q-3 · tier=quick");
  expect(renderResultText(receipt("committed", {}, { replayed: true }), false, false, plain)).toBe("↺ replayed");
  expect(renderResultText(receipt("rejected", { code: "gate_not_current", reason: "rebase first" }), false, false, theme))
    .toBe("<error>✗ rejected  gate_not_current: rebase first</>");
  expect(renderResultText(receipt("uncertain", { code: "gate_start_uncertain" }), false, false, plain))
    .toBe("◷ uncertain gate_start_uncertain");
});

test("a call without a receipt shows its error, and only the expanded view shows the raw result", () => {
  const failed: AgentToolResult<unknown> = { content: [{ type: "text", text: "call_timeout: exceeded" }], details: undefined };
  expect(renderResultText(failed, false, true, plain)).toBe("✗ call_timeout: exceeded");
  const collapsed = renderResultText(receipt("committed", { key: "Q-1" }), false, false, plain);
  const expanded = renderResultText(receipt("committed", { key: "Q-1" }), true, false, plain);
  expect(collapsed.split("\n")).toHaveLength(1);
  expect(expanded).toContain('"key": "Q-1"');
});

test("the renderers draw width-bounded components", () => {
  const renderers = corralRenderers("corral_ask");
  const context = { isError: false } as Parameters<NonNullable<typeof renderers.renderResult>>[3];
  const call = renderers.renderCall?.({ kind: "clarification", text: "a ".repeat(100) }, plain as never, context as never);
  const lines = call?.render(40) ?? [];
  expect(lines.length).toBeGreaterThan(1);
  for (const line of lines) expect(line.length).toBeLessThanOrEqual(40);
  const result = renderers.renderResult?.(receipt("committed", { key: "Q-9" }), { expanded: false } as never, plain as never, context);
  expect(result?.render(40).join("").trim()).toBe("✓ key=Q-9");
});

test("the header names the role, the run and the model; the title is short", () => {
  const worker = { store: "s", project: "corral", role: "worker" as const, task: "T-503", run: "R1", generation: 2 };
  const lines = headerLines({ identity: worker, provider: "openai-codex", model: "gpt-6.1-sol", effort: "high", cwd: "/w/T-503" }, plain);
  expect(lines).toEqual(["▌WORKER  T-503 · R1  g2", "openai-codex/gpt-6.1-sol · high · T-503"]);
  expect(titleText(worker)).toBe("T-503 worker");
  expect(target({ ...worker, role: "reviewer", review: "R1-review1" })).toBe("T-503 · R1-review1");
  const pm = { store: "s", project: "corral", role: "pm" as const, generation: 1 };
  expect(target(pm)).toBe("corral");
  expect(titleText(pm)).toBe("corral pm");
  expect(headerLines({ identity: pm, provider: "anthropic", model: "anthropic/claude-opus-5-5", cwd: "/w" }, plain)[1])
    .toBe("anthropic/claude-opus-5-5 · w");
});

test("the Dracula theme builds through Pi's Theme class and paints its palette", () => {
  const dracula = draculaTheme(Theme, "truecolor");
  expect(dracula.name).toBe("dracula");
  expect(dracula.appearance).toBe("dark");
  expect(dracula.fg("accent", "x")).toContain("38;2;189;147;249");
  expect(dracula.fg("error", "x")).toContain("38;2;255;85;85");
  expect(dracula.bg("selectedBg", "x")).toContain("48;2;68;71;90");
  expect(DRACULA.purple).toBe("#bd93f9");
  expect(draculaTheme(Theme, "256color").getColorMode()).toBe("256color");
});

test("the footer shows the binding always and refusals only once they happened", () => {
  expect(bindingStatus(true, 3, plain)).toBe("● corral g3");
  expect(bindingStatus(false, 3, plain)).toBe("○ corral unbound");
  const alerts: Alerts = { networkRefused: 0, toolsDenied: 0 };
  expect(alertsStatus(alerts, plain)).toBeUndefined();
  const events = new EventLog(1);
  events.on((event) => countAlert(alerts, event));
  events.emit("network_refused", { destination: "https://example.com/" });
  events.emit("tool_denied", { tool: "web" });
  events.emit("tool_denied", { tool: "web" });
  events.emit("queue", { steering: 0 });
  expect(alertsStatus(alerts, plain)).toBe("⚠ 1 network refused · 2 tool denied");
});
