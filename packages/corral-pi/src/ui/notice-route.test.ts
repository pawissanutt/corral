import { expect, test } from "bun:test";
import type { StatusNotice } from "@earendil-works/pi-coding-agent";

import { noticeRoute, statusPresenter, type NoticeSink, type NoticeTone } from "./notice-route.ts";

interface Told { readonly text: string; readonly tone: NoticeTone | undefined; readonly detail: string | undefined }
interface Row { readonly text: string; readonly tone: string; render(width: number): string[]; invalidate(): void }

/** The hook with a recording sink (or none) and rows that keep what they were given. */
function hook(options: { inChat?: boolean; sink?: "none" | "throws" } = {}) {
  const told: Told[] = [];
  const sink: NoticeSink | undefined = options.sink === "none" ? undefined : {
    notice(text, tone, detail) {
      if (options.sink === "throws") throw new Error("disposed");
      told.push({ text, tone, detail });
    },
  };
  const present = statusPresenter({
    sink: () => sink,
    inChat: () => options.inChat ?? true,
    row: (text, tone): Row => ({ text, tone, render: () => [text], invalidate() {} }),
  });
  return { told, present: (notice: StatusNotice) => present(notice) as Row | true | undefined };
}
const pi = (text: string, level: StatusNotice["level"], topic?: StatusNotice["topic"]): StatusNotice =>
  ({ text, level, origin: "pi", ...(topic ? { topic } : {}) });

test("without a sink, or with nothing past its framing, Pi keeps its own line", () => {
  expect(hook({ sink: "none" }).present(pi("Reload failed: ENOENT", "error"))).toBeUndefined();
  const { told, present } = hook();
  const blank: StatusNotice = { text: "corral: Error: ", level: "error", origin: "extension" };
  expect(noticeRoute(blank, { inChat: true })).toBe("pi");
  expect(present(blank)).toBeUndefined();
  expect(told).toEqual([]);
});

test("an error is a notice and a quiet row, without its prefix, and the exact raw text is kept", () => {
  const raw = "corral: Error: session export failed\n  at write (/tmp/out.jsonl)";
  const { told, present } = hook();
  const row = present({ text: raw, level: "error", origin: "extension" });
  expect(told).toEqual([{ text: "session export failed", tone: "error", detail: raw }]);
  expect(row !== true && row?.text).toBe("session export failed\n  at write (/tmp/out.jsonl)");
  expect(row !== true && row?.tone).toBe("error");

  // A warning is a notice only; a prefix alone is framing, not a detail.
  const warned = hook();
  expect(warned.present({ text: "corral: herdr is not reachable", level: "warning", origin: "extension" })).toBe(true);
  expect(warned.told).toEqual([{ text: "herdr is not reachable", tone: "warning", detail: undefined }]);
});

test("a model or effort line is a notice only, even a refusal", () => {
  const { told, present } = hook();
  expect(present(pi("Model gpt-6.1-sol is not allowed for this role", "error", "model"))).toBe(true);
  expect(present(pi("Thinking level: high", "info", "effort"))).toBe(true);
  expect(told.map(entry => entry.tone)).toEqual(["error", "text"]);
});

test("a compaction outcome is a row in Chat and also a notice elsewhere, manual and automatic told apart", () => {
  const chat = hook();
  const cancelled = chat.present(pi("Compaction cancelled", "error", "compaction"));
  expect(chat.told).toEqual([]);
  expect(cancelled !== true && cancelled?.text).toBe("compaction cancelled · context unchanged");
  expect(cancelled !== true && cancelled?.tone).toBe("text");

  const away = hook({ inChat: false });
  const auto = away.present(pi("Auto-compaction cancelled", "info", "compaction"));
  expect(away.told).toEqual([{ text: "Automatic compaction cancelled · context unchanged", tone: "text", detail: undefined }]);
  expect(auto !== true && auto?.text).toBe("automatic compaction cancelled · context unchanged");

  const failed = away.present(pi("Auto-compaction failed: 529 overloaded", "error", "compaction"));
  expect(away.told[1]).toEqual({ text: "Automatic compaction failed · context unchanged", tone: "error", detail: "Auto-compaction failed: 529 overloaded" });
  expect(failed !== true && failed?.text.split("\n")).toEqual([
    "automatic compaction failed · context unchanged", "Auto-compaction failed: 529 overloaded", "Send /compact to try again.",
  ]);
  // A retry cannot change a settled reason, so no next step is offered.
  const settled = chat.present(pi("Compaction failed: Nothing to compact (session too small)", "error", "compaction"));
  expect(settled !== true && settled?.text).not.toContain("/compact");
});

test("a sink that fails loses nothing: a notice falls back to Pi's line, an error keeps its row", () => {
  const { present } = hook({ sink: "throws" });
  expect(present(pi("Copied selected message to clipboard", "info"))).toBeUndefined();
  const row = present(pi("Failed to send queued message: offline", "error"));
  expect(row !== true && row?.text).toBe("Failed to send queued message: offline");
});
