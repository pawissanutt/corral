import { expect, test } from "bun:test";
import { resolveClaudeCode } from "../src/claude/sdk.ts";

test("a missing Claude Code platform package is refused with a stable code naming it", () => {
  const deps = { platform: "linux", arch: "x64", resolve: () => { throw new Error("not found"); }, isFile: () => false };
  expect(() => resolveClaudeCode(deps)).toThrow(/^claude_code_unavailable: .*install @anthropic-ai\/claude-agent-sdk-linux-x64 /);
  // The shipped install resolves the binary the SDK's platform package put next to it.
  expect(resolveClaudeCode()).toMatch(/@anthropic-ai\/claude-agent-sdk-linux-x64(-musl)?\/claude$/);
});

test("only a compiled host, which carries no platform package, falls back to claude on PATH", () => {
  const missing = { platform: "linux", arch: "x64", resolve: () => { throw new Error("not found"); } };
  const onPath = (file: string) => file === "/opt/claude/bin/claude";
  const compiled = { ...missing, compiled: true, path: "/usr/bin:/opt/claude/bin", isFile: onPath };
  expect(resolveClaudeCode(compiled)).toBe("/opt/claude/bin/claude");
  expect(() => resolveClaudeCode({ ...compiled, path: "/usr/bin" }))
    .toThrow(/^claude_code_unavailable: .*install the `claude` CLI/);
  expect(() => resolveClaudeCode({ ...missing, path: "/opt/claude/bin", isFile: onPath }))
    .toThrow(/install @anthropic-ai\/claude-agent-sdk-linux-x64 /);
});
