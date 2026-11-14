import { expect, test } from "bun:test";
import { claudeEnvironment, claudeMaxContextTokens } from "./stream.ts";

test("Pi-owned compaction keeps Claude Code's reminder, compaction and auto memory off", () => {
  const env = claudeEnvironment({ PATH: "/bin", CLAUDE_CODE_TOTAL_TOKENS_REMINDER: "on",
    CLAUDE_CODE_MAX_CONTEXT_TOKENS: "1", CLAUDE_CODE_DISABLE_AUTO_MEMORY: "0" }, "token", "/private", "1h",
    { id: "claude-sonnet-5-5", contextWindow: 1_000_000 });
  expect(env.CLAUDE_CODE_TOTAL_TOKENS_REMINDER).toBe("off");
  expect(env.DISABLE_COMPACT).toBe("1");
  expect(env.CLAUDE_CODE_DISABLE_AUTO_MEMORY).toBe("1");
  // Claude Code's local refusal lands on the window Pi compacts against, not 23k below it.
  expect(env.CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBe("1023000");
  expect(claudeEnvironment({ PATH: "/bin" }, "token", "/private").CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBeUndefined();
});

test("legacy long-context ids are planned at the plain 200k window this lane requests", () => {
  expect(claudeMaxContextTokens({ id: "claude-sonnet-4-6", contextWindow: 1_000_000 })).toBe(223_000);
  expect(claudeMaxContextTokens({ id: "claude-opus-5-5", contextWindow: 1_000_000 })).toBe(1_023_000);
});

test("Claude fine-grained streaming overrides inherited Claude Code settings", () => {
  const env = claudeEnvironment({ PATH: "/bin", CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING: "0",
    CLAUDE_CODE_USE_BEDROCK: "1" }, "token", "/private");
  expect(env.CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING).toBe("1");
  expect(env.CLAUDE_CODE_USE_BEDROCK).toBeUndefined();
  expect(env.PATH).toBe("/bin");
});
