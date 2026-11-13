import { expect, test } from "bun:test";
import { claudeEnvironment } from "./stream.ts";

test("Claude fine-grained streaming overrides inherited Claude Code settings", () => {
  const env = claudeEnvironment({ PATH: "/bin", CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING: "0",
    CLAUDE_CODE_USE_BEDROCK: "1" }, "token", "/private");
  expect(env.CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING).toBe("1");
  expect(env.CLAUDE_CODE_USE_BEDROCK).toBeUndefined();
  expect(env.PATH).toBe("/bin");
});
