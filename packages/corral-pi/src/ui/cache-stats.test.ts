import { expect, test } from "bun:test";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { getModel } from "@earendil-works/pi-ai/compat";
import { emptyOutput } from "../claude/events.ts";
import { cacheStatsText } from "./cache-stats.ts";

test("cache rates distinguish the latest request from weighted session input including compaction", () => {
  const model = getModel("anthropic", "claude-sonnet-5-5");
  if (!model) throw new Error("missing model fixture");
  const first = emptyOutput(model), latest = emptyOutput(model);
  first.usage = { ...first.usage, input: 10, cacheRead: 80, cacheWrite: 10, output: 200 };
  latest.usage = { ...latest.usage, input: 0, cacheRead: 90, cacheWrite: 10, output: 300 };
  const base = { parentId: null, timestamp: "2026-01-01T00:00:00Z" };
  const entries: SessionEntry[] = [
    { ...base, type: "message", id: "first", message: first },
    { ...base, type: "compaction", id: "compact", firstKeptEntryId: "first", summary: "summary", tokensBefore: 100,
      usage: { ...emptyOutput(model).usage, input: 20 } },
    { ...base, type: "message", id: "nested", message: { role: "toolResult", toolName: "explore", toolCallId: "call",
      content: [], isError: false, timestamp: 0, usage: { ...emptyOutput(model).usage, input: 10 } } },
    { ...base, type: "usage", id: "usage", kind: "cache_warm", provider: "anthropic", model: model.id,
      usage: { ...emptyOutput(model).usage, cacheRead: 30 } },
    { ...base, type: "message", id: "latest", message: latest },
  ];
  const text = cacheStatsText(entries);
  expect(text).toContain("Latest model request: 90.0%");
  expect(text).toContain("Whole session: 76.9%");
  expect(text).toContain("40 uncached, 20 written, 200 read");
  expect(cacheStatsText([])).toContain("Whole session: unavailable");
});
