import { expect, test } from "bun:test";

import { Theme } from "@earendil-works/pi-coding-agent";

import { EventLog } from "../bridge/events.ts";
import {
  type Alerts, alertsStatus, bindingStatus, countAlert, headerLines, target, titleText,
} from "./chrome.ts";
import { DRACULA, draculaTheme } from "./dracula.ts";

const plain = { fg: (_color: string, text: string) => text, bold: (text: string) => text };

test("the header names the role, the run and the model; the title is short", () => {
  const worker = { store: "s", project: "corral", role: "worker" as const, task: "T-503", run: "R1" };
  const lines = headerLines({ identity: worker, provider: "openai-codex", model: "gpt-6.1-sol", effort: "high", cwd: "/w/T-503" }, plain);
  expect(lines).toEqual(["▌WORKER  T-503 · R1", "openai-codex/gpt-6.1-sol · high · T-503"]);
  expect(titleText(worker)).toBe("T-503 worker");
  expect(target({ ...worker, role: "reviewer", run: "R2" })).toBe("T-503 · R2");
  const assistant = { store: "s", project: "corral", role: "assistant" as const, assistant: "A-4" };
  expect(target(assistant)).toBe("A-4");
  expect(titleText(assistant)).toBe("corral assistant");
  expect(headerLines({ identity: assistant, provider: "anthropic", model: "anthropic/claude-opus-5-5", cwd: "/w" }, plain)[1])
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
  expect(bindingStatus(true, plain)).toBe("● corral bound");
  expect(bindingStatus(false, plain)).toBe("○ corral unbound");
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
