import { expect, test } from "bun:test";
import type { Message, ToolResultMessage } from "@earendil-works/pi-ai";

import { buildPromptBlocks } from "../src/claude/prompt.ts";
import { claudeEnvironment, claudeSystemPrompt } from "../src/claude/stream.ts";

const user = (content: string) => ({ role: "user" as const, content, timestamp: 0 });
const result = (content: ToolResultMessage["content"]): ToolResultMessage => ({
  role: "toolResult", toolCallId: "read-1", toolName: "read", content, isError: false, timestamp: 0,
});

test("the system prompt keeps Claude Code's preset and appends only corral's sections, without Pi's harness identity", () => {
  const system = {
    role: "system" as const, content: "", timestamp: 0,
    sections: {
      preamble: "You are an expert coding assistant operating inside pi, a coding agent harness.",
      tools: "<tools>- read</tools>", rules: "<rules>- Use read</rules>", docs: "<docs>Pi documentation</docs>",
      addendum: "<addendum>\n# corral worker contract (corral-pi)\nRun the gate.\n</addendum>", cwd: "<cwd>\n/w\n</cwd>",
    },
  };
  expect(claudeSystemPrompt([system])).toEqual({
    type: "preset", preset: "claude_code",
    append: "<addendum>\n# corral worker contract\nRun the gate.\n</addendum>\n\n<cwd>\n/w\n</cwd>",
  });
});

test("question-state changes are live context, not mutations of the cached system prefix", () => {
  const context = (state: string) => [{ role: "system" as const, content: "", timestamp: 0,
    sections: { addendum: "Permanent authority rules", owner_questions: state } }, user("continue")];
  const pending = context("A question is pending; this is not permission");
  const answered = context("Answer delivery completed; consult the owner input");
  expect(claudeSystemPrompt(pending)).toEqual(claudeSystemPrompt(answered));
  expect(JSON.stringify(buildPromptBlocks(pending, new Map()))).toContain("A question is pending");
  expect(JSON.stringify(buildPromptBlocks(answered, new Map()))).toContain("Answer delivery completed");
});

test("Claude replay caches an unchanged text prefix before its moving suffix, with the SDK's TTL", () => {
  const env = claudeEnvironment({ CLAUDE_CODE_PROMPT_CACHE_TTL: "1h" }, "synthetic", "/private");
  const cases: Message[][] = [
    [user("inspect"), result([{ type: "text", text: "file contents" }])],
    [user("inspect"), result([{ type: "text", text: "caption" },
      { type: "image", mimeType: "image/png", data: "aW1hZ2U=" }])],
    [user("inspect"), result([{ type: "image", mimeType: "image/png", data: "aW1hZ2U=" }])],
    [user("inspect"), result([{ type: "text", text: "" }])],
  ];
  for (const history of cases) {
    const first = buildPromptBlocks([...history, user("first question")], new Map());
    const next = buildPromptBlocks([...history, user("first question"),
      result([{ type: "text", text: "new result" }]), user("second question")], new Map());
    const markers = first.flatMap((block, index) => "cache_control" in block ? [index] : []);
    expect(markers).toHaveLength(1);
    const at = markers[0];
    if (at === undefined) throw new Error("history breakpoint missing");
    expect(first[at]).toMatchObject({ type: "text", cache_control: { type: "ephemeral", ttl: "5m" } });
    expect(env.CLAUDE_CODE_PROMPT_CACHE_TTL).toBe("5m");
    // Marker metadata may move; the content prefix at the old write position must not.
    const plain = (blocks: typeof first) => blocks.map((block) => block.type === "text"
      ? { type: block.type, text: block.text } : block);
    expect(plain(next.slice(0, at + 1))).toEqual(plain(first.slice(0, at + 1)));
    expect(first.slice(0, at + 1).some((block) => block.type === "text" && block.text.includes("</conversation_history>")))
      .toBe(false);
    expect(first.at(-1)).toEqual({ type: "text", text: "first question" });
  }
  // Removing a custom tool cannot change the bytes of its historical results.
  const removed: Message[] = [result([{ type: "text", text: "answer" }]), user("continue")];
  const entry = removed[0];
  if (entry?.role !== "toolResult") throw new Error("missing result");
  entry.toolName = "lookup_notes";
  expect(buildPromptBlocks(removed, new Map([["lookup_notes", "mcp__custom-tools__lookup_notes"]])))
    .toEqual(buildPromptBlocks(removed, new Map()));
  // A cold request has no historical prefix to mark.
  expect(buildPromptBlocks([user("hello")], new Map()).some((block) => "cache_control" in block)).toBe(false);
});
