import { expect, test } from "bun:test";
import { getModel } from "@earendil-works/pi-ai/compat";
import { ReplayCache } from "../src/claude/cache.ts";
import { resolveCacheRetention } from "../src/claude/cache-policy.ts";
import { emptyOutput, updateUsage } from "../src/claude/events.ts";
import type { PromptBlock } from "../src/claude/prompt.ts";

function history(size: number, ttl: "5m" | "1h" = "5m"): PromptBlock[] {
  return Array.from({ length: size }, (_, index) => ({ type: "text", text: `block ${index}`,
    ...(index === size - 1 ? { cache_control: { type: "ephemeral", ttl } as const } : {}) }));
}

test("retention rejects unknown configuration instead of silently choosing a lifetime", () => {
  expect(resolveCacheRetention(undefined, undefined)).toBe("5m");
  expect(() => resolveCacheRetention("forever", undefined)).toThrow("claude_cache_retention_invalid");
  expect(resolveCacheRetention("1h", "none")).toBe("none");
});

test("expired writes, changed prefixes and late completions cannot pin a stale frontier", () => {
  for (const ttl of ["5m", "1h"] as const) {
    const cache = new ReplayCache();
    const lifetime = ttl === "5m" ? 300_000 : 3_600_000;
    cache.prepare(history(2, ttl), "session", 0).commit();
    const expired = cache.prepare(history(35, ttl), "session", lifetime);
    expect(expired.observation.localState).toBe("expired");
    expect(expired.observation.uncachedTailBlocks).toBe(0);
    const unconfirmed = cache.prepare(history(35, ttl), "session", 1);
    expect(unconfirmed.observation.cachedThroughBlock).toBe(21);
    const retry = cache.prepare(history(35, ttl), "session", 2);
    expect(retry.observation.cachedThroughBlock).toBe(21);
    retry.commit();
    unconfirmed.commit();
    expect(cache.prepare(history(35, ttl), "session", 3).observation.sinceConfirmedStartMs).toBe(1);
    const compacted = history(3, ttl);
    compacted[0] = { type: "text", text: "canonical summary" };
    expect(cache.prepare(compacted, "session", 4).observation.localState).toBe("changed");
    expect(cache.prepare(history(35, ttl), "other-session", 4).observation.localState).toBe("cold");
  }
});

test("reported one-hour writes retain their tier and are not priced as five-minute writes", () => {
  const catalog = getModel("anthropic", "claude-sonnet-5-5");
  if (!catalog) throw new Error("missing model fixture");
  const { compat: _compat, ...metadata } = catalog;
  const model = { ...metadata, cost: { input: 1, output: 5, cacheRead: 0.1, cacheWrite: 1.25 } };
  const output = emptyOutput(model);
  updateUsage(model, output, { input_tokens: 0, output_tokens: 0, cache_creation_input_tokens: 10,
    cache_creation: { ephemeral_1h_input_tokens: 10 } });
  expect(output.usage.cacheWrite1h).toBe(10);
  expect(output.usage.cost.cacheWrite).toBe(0.00002);
  expect(output.usage.totalTokens).toBe(10);
});
